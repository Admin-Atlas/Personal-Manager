// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Handing a blocking, fallible closure to the tokio blocking pool, in one place.
//!
//! Almost every command with a slow or DB-touching half runs it through `spawn_blocking`, and each
//! site used to hand-roll the same three lines to turn a `JoinError` — the task panicked — into a
//! [`crate::error::Error`]. That is the duplication; `label` is why it could not simply be deleted.
//! The ~40 copies carried ~30 DISTINCT diagnostics ("transcription task panicked", "migration task
//! panicked", …), and a helper with the string baked in would have collapsed all of them into one
//! wrong sentence. Taking the label keeps every site's own message byte-for-byte, so adopting this
//! is a no-op the user can never observe.
//!
//! Two variants, and the difference between them is load-bearing rather than stylistic:
//!
//! - [`spawn_blocking_result`] **flattens**: a panic and a returned `Err` both arrive as one `Err`.
//!   Right for the sites that do nothing with the outcome but `?` it.
//! - [`spawn_blocking_join`] **does not flatten**: it maps only the `JoinError` and hands back the
//!   closure's own `Result` untouched. The backup/restore commands bind that inner result and
//!   inspect it — to emit `BackupEvent::Failed`, and to relabel the error as "Backup cancelled."
//!   when the user pressed Cancel. Flattening there would route a task PANIC into that arm and
//!   report a crash to the user as a cancellation, so those five sites take this one instead.
//!
//! Not every `spawn_blocking` in the tree fits either shape, and the ones that do not are left
//! hand-rolled with a comment saying why (a closure returning `Option`, a message that says
//! "failed" rather than "panicked", and the grounding task whose whole point is that the two layers
//! stay distinguishable). Those comments exist so nobody "finishes the job" later.

use crate::error::{Error, Result};

/// Run a blocking, fallible closure on the blocking pool and FLATTEN the `JoinError` into the
/// closure's own error type. A panic surfaces as `Error::Other("{label} task panicked: …")`; an
/// `Err` the closure returned surfaces unchanged.
pub(crate) async fn spawn_blocking_result<T, F>(label: &'static str, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Other(format!("{label} task panicked: {e}")))?
}

/// As [`spawn_blocking_result`], but keeps the two layers APART: only the `JoinError` is mapped, and
/// the closure's own `Result` comes back intact for the caller to inspect.
///
/// Use this wherever the inner result is examined rather than propagated — the backup and restore
/// commands read it to decide whether to emit `BackupEvent::Failed` and whether the failure was
/// really the user's Cancel. Flattening would make a panic indistinguishable from those, which is
/// the one way this refactor could have changed what a user sees.
pub(crate) async fn spawn_blocking_join<T, F>(label: &'static str, f: F) -> Result<Result<T>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Other(format!("{label} task panicked: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_label_is_what_keeps_every_call_sites_own_message() {
        // The whole reason the helper takes a label: ~30 sites each had their own panic string, and a
        // helper with one baked in would have silently retyped all of them.
        let err = spawn_blocking_result("transcription", || -> Result<()> { panic!("boom") })
            .await
            .expect_err("a panicking task must not read as success");
        assert!(
            err.to_string().starts_with("transcription task panicked: "),
            "expected the site's own label, got {err}"
        );
    }

    #[tokio::test]
    async fn spawn_blocking_result_flattens_but_never_relabels_the_closures_own_error() {
        let err = spawn_blocking_result("widget", || -> Result<()> {
            Err(Error::Other("the disk is full".into()))
        })
        .await
        .expect_err("the closure's Err must still surface");
        assert_eq!(err.to_string(), "the disk is full");
    }

    #[tokio::test]
    async fn spawn_blocking_join_keeps_a_panic_and_a_returned_error_distinguishable() {
        // The property the five backup/restore sites depend on: an inner `Err` stays INSIDE the Ok
        // arm, so the caller's `Failed`/"cancelled" handling is reached only by a real failure — a
        // panic comes back as the outer `Err` and never wears the cancellation label.
        let inner = spawn_blocking_join("backup", || -> Result<()> {
            Err(Error::Other("the archive is corrupt".into()))
        })
        .await
        .expect("a returned Err is not a join failure");
        assert_eq!(
            inner
                .expect_err("the closure's Err belongs in the Ok arm")
                .to_string(),
            "the archive is corrupt"
        );

        let joined = spawn_blocking_join("backup", || -> Result<()> { panic!("boom") }).await;
        assert!(joined
            .expect_err("a panic must surface as the OUTER error")
            .to_string()
            .starts_with("backup task panicked: "));
    }
}
