// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `pmmod` scheme's response headers (Mods spec §1.4, "What the design guarantees"). Tauri adds
//! a CSP only to its own assets, so the handler sets every security header itself.
//!
//! Like `engine.rs`, this file names nothing in PM's crate (std only), so the sealed-frame
//! regression app can include it with `#[path]` and measure exactly the headers PM sends.
//!
//! - The CSP's fixed directives come first, because under CSP the first copy of a directive wins: a
//!   later `connect-src` can't loosen the fixed one. Its `sandbox` directive forces an opaque origin
//!   even if the frame lost its `sandbox` attribute.
//! - `script-src`, `style-src`, `img-src` and `font-src` are the mod's own `app/` folder, built from
//!   its validated id. No `blob:` for scripts and no `'unsafe-eval'`.
//! - `frame-ancestors` is PM's own page, so nothing else can frame a mod.
//! - An HTML, XHTML, XML or SVG file is a document only as the one-time entry the frame is loading;
//!   any other response of those types (and of any type not listed here) goes out as an attachment,
//!   which WebKitGTK and WebView2 turn into a download that PM's download refusal then drops. An
//!   `<img>` still loads an SVG. WKWebView doesn't: wry's response delegate there shows any type
//!   WebKit can display whatever its `Content-Disposition` says (read from wry 0.57.1, not measured),
//!   so on macOS the navigation guard alone keeps a non-entry document out of a frame. It can, since
//!   a `pmmod` URL always parses and so always reaches the guard.

/// Directives no mod can change, first in the policy.
pub const FIXED_DIRECTIVES: &str =
    "sandbox allow-scripts; default-src 'none'; connect-src 'none'; \
     frame-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; worker-src blob:";

/// Every feature off. An engine that doesn't know a feature ignores it (Chromium logs a warning).
pub const PERMISSIONS_POLICY: &str = "accelerometer=(), ambient-light-sensor=(), \
     attribution-reporting=(), autoplay=(), bluetooth=(), browsing-topics=(), camera=(), \
     clipboard-read=(), clipboard-write=(), compute-pressure=(), display-capture=(), \
     encrypted-media=(), fullscreen=(), gamepad=(), geolocation=(), gyroscope=(), hid=(), \
     identity-credentials-get=(), idle-detection=(), local-fonts=(), magnetometer=(), \
     microphone=(), midi=(), otp-credentials=(), payment=(), picture-in-picture=(), \
     publickey-credentials-create=(), publickey-credentials-get=(), screen-wake-lock=(), \
     serial=(), speaker-selection=(), storage-access=(), usb=(), web-share=(), \
     window-management=(), xr-spatial-tracking=()";

/// The folder a mod's served files live in, inside its package.
pub const APP_DIR: &str = "app/";

/// How a response goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The one-time entry page, rendered in the frame.
    Document,
    /// A script, style, image, font or data file the page loads.
    Subresource,
    /// Anything that could render as a document but isn't the entry, and anything of a type not
    /// listed: `Content-Disposition: attachment`.
    Attachment,
}

/// The `Content-Type` for a package path, by its extension, and whether that type can render as a
/// document. `None` for an extension this table doesn't know.
pub fn content_type(path: &str) -> Option<(&'static str, bool)> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" => ("text/html; charset=utf-8", true),
        "xhtml" | "xht" => ("application/xhtml+xml", true),
        "xml" | "xsl" | "xslt" => ("application/xml", true),
        "svg" => ("image/svg+xml", true),
        "js" | "mjs" => ("text/javascript; charset=utf-8", false),
        "css" => ("text/css; charset=utf-8", false),
        "json" | "map" => ("application/json", false),
        "wasm" => ("application/wasm", false),
        "png" => ("image/png", false),
        "jpg" | "jpeg" => ("image/jpeg", false),
        "gif" => ("image/gif", false),
        "webp" => ("image/webp", false),
        "avif" => ("image/avif", false),
        "ico" => ("image/x-icon", false),
        "woff2" => ("font/woff2", false),
        "woff" => ("font/woff", false),
        "ttf" => ("font/ttf", false),
        "otf" => ("font/otf", false),
        "txt" | "md" => ("text/plain; charset=utf-8", false),
        _ => return None,
    })
}

/// How a file goes out: a document only when it is the entry being consumed and an HTML page.
pub fn disposition(path: &str, is_entry: bool) -> Disposition {
    match content_type(path) {
        Some((mime, true)) if is_entry && mime.starts_with("text/html") => Disposition::Document,
        Some((_, false)) => Disposition::Subresource,
        _ => Disposition::Attachment,
    }
}

/// The mod's own folder as a CSP source, `<mod origin>/<id>/app/`. `None` unless the id and the
/// origin hold only characters that can't end a source expression or start a new directive, so a
/// caller passing an unvalidated id can't inject policy.
pub fn mod_source(mod_origin: &str, id: &str) -> Option<String> {
    let id_ok = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    (id_ok && is_origin(mod_origin)).then(|| format!("{mod_origin}/{id}/{APP_DIR}"))
}

/// `scheme://host[:port]` in the characters PM's origins use, with nothing after the host.
fn is_origin(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    !scheme.is_empty()
        && scheme.bytes().all(|b| b.is_ascii_lowercase())
        && !rest.is_empty()
        && rest.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-' || b == b':'
        })
}

/// The whole policy for one mod. `None` when the id, the mod origin or an ancestor isn't safe to
/// put in a header (see [`mod_source`]).
pub fn csp(mod_origin: &str, id: &str, frame_ancestors: &[&str]) -> Option<String> {
    let base = mod_source(mod_origin, id)?;
    if !frame_ancestors.iter().all(|a| is_origin(a)) {
        return None;
    }
    let ancestors = if frame_ancestors.is_empty() {
        "'none'".to_owned()
    } else {
        frame_ancestors.join(" ")
    };
    Some(format!(
        "{FIXED_DIRECTIVES}; script-src {base}; style-src {base}; img-src {base}; font-src {base}; \
         frame-ancestors {ancestors}"
    ))
}

/// Every header for one served file, in order. `None` when the CSP can't be built (see [`csp`]).
pub fn file_headers(
    mod_origin: &str,
    id: &str,
    path: &str,
    frame_ancestors: &[&str],
    is_entry: bool,
) -> Option<Vec<(&'static str, String)>> {
    let policy = csp(mod_origin, id, frame_ancestors)?;
    let disposition = disposition(path, is_entry);
    let mime = content_type(path).map_or("application/octet-stream", |(mime, _)| mime);
    let mut headers = vec![
        ("Content-Type", mime.to_owned()),
        ("Content-Security-Policy", policy),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("X-DNS-Prefetch-Control", "off".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Permissions-Policy", PERMISSIONS_POLICY.to_owned()),
    ];
    if disposition == Disposition::Attachment {
        headers.push(("Content-Disposition", "attachment".to_owned()));
    }
    Some(headers)
}

/// The headers on a refusal (an empty body): an empty sandboxed document if anything renders it.
pub fn refusal_headers() -> Vec<(&'static str, String)> {
    vec![
        (
            "Content-Security-Policy",
            "sandbox; default-src 'none'; frame-ancestors 'none'".to_owned(),
        ),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEBKIT: &str = "pmmod://localhost";
    const WEBVIEW2: &str = "http://pmmod.localhost";

    fn directives(policy: &str) -> Vec<&str> {
        policy.split(';').map(str::trim).collect()
    }

    #[test]
    fn the_fixed_directives_come_first_and_the_mods_path_follows() {
        let policy = csp(WEBKIT, "atlas.map", &["tauri://localhost"]).unwrap();
        let d = directives(&policy);
        assert_eq!(
            d,
            [
                "sandbox allow-scripts",
                "default-src 'none'",
                "connect-src 'none'",
                "frame-src 'none'",
                "form-action 'none'",
                "base-uri 'none'",
                "object-src 'none'",
                "worker-src blob:",
                "script-src pmmod://localhost/atlas.map/app/",
                "style-src pmmod://localhost/atlas.map/app/",
                "img-src pmmod://localhost/atlas.map/app/",
                "font-src pmmod://localhost/atlas.map/app/",
                "frame-ancestors tauri://localhost",
            ]
        );
        // Each directive once, so a later copy can't be the one that wins.
        let mut names: Vec<&str> = d.iter().map(|x| x.split(' ').next().unwrap()).collect();
        names.sort_unstable();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len);
        assert!(!policy.contains("unsafe"));
        // `blob:` only for workers: a blob script would run fetched or stored text.
        assert_eq!(d.iter().filter(|x| x.contains("blob:")).count(), 1);
    }

    #[test]
    fn webview2_gets_its_own_origin_and_dev_ancestors_are_listed() {
        let policy = csp(
            WEBVIEW2,
            "acme.tool",
            &["http://tauri.localhost", "http://localhost:1420"],
        )
        .unwrap();
        assert!(policy.contains("script-src http://pmmod.localhost/acme.tool/app/;"));
        assert!(policy.ends_with("frame-ancestors http://tauri.localhost http://localhost:1420"));
        assert!(csp(WEBKIT, "a.b", &[])
            .unwrap()
            .ends_with("frame-ancestors 'none'"));
    }

    #[test]
    fn an_id_or_origin_that_could_inject_policy_builds_no_header() {
        for id in [
            "",
            "a.b; script-src *",
            "a.b *",
            "a.b'",
            "A.b",
            "a/b",
            "a.b,c",
            "a.b\n",
            "a.b\r\nX-Evil: 1",
        ] {
            assert_eq!(csp(WEBKIT, id, &[]), None, "{id:?}");
            assert_eq!(file_headers(WEBKIT, id, "app/x.js", &[], false), None);
        }
        for origin in [
            "",
            "pmmod:",
            "pmmod://",
            "pmmod://localhost/",
            "pmmod://l h",
            "x://a;b",
        ] {
            assert_eq!(csp(origin, "a.b", &[]), None, "{origin:?}");
        }
        for ancestor in [
            "*",
            "'self'",
            "tauri://localhost; script-src *",
            "http://a b",
        ] {
            assert_eq!(csp(WEBKIT, "a.b", &[ancestor]), None, "{ancestor:?}");
        }
    }

    #[test]
    fn documents_render_only_as_the_entry() {
        assert_eq!(disposition("app/index.html", true), Disposition::Document);
        for path in [
            "app/index.html",
            "app/x.htm",
            "app/x.xhtml",
            "app/x.xml",
            "app/x.svg",
            "app/x.XSL",
        ] {
            assert_eq!(disposition(path, false), Disposition::Attachment, "{path}");
        }
        // An entry flag on something that isn't HTML doesn't make it a document.
        assert_eq!(disposition("app/x.svg", true), Disposition::Attachment);
        assert_eq!(disposition("app/x.js", true), Disposition::Subresource);
        for path in [
            "app/x.pdf",
            "app/x",
            "app/x.exe",
            "app/.htaccess",
            "app/x.html.bak",
        ] {
            assert_eq!(disposition(path, false), Disposition::Attachment, "{path}");
        }
        for path in [
            "app/main.js",
            "app/m.mjs",
            "app/s.css",
            "app/i.png",
            "app/f.woff2",
            "app/d.json",
            "app/n.txt",
            "app/a.JPG",
        ] {
            assert_eq!(disposition(path, false), Disposition::Subresource, "{path}");
        }
    }

    fn header<'a>(h: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
        h.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }

    #[test]
    fn every_file_carries_the_security_headers() {
        let h = file_headers(
            WEBKIT,
            "atlas.map",
            "app/main.js",
            &["tauri://localhost"],
            false,
        )
        .unwrap();
        assert_eq!(
            header(&h, "Content-Type"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(header(&h, "X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(header(&h, "X-DNS-Prefetch-Control"), Some("off"));
        assert_eq!(header(&h, "Cache-Control"), Some("no-store"));
        assert_eq!(header(&h, "Permissions-Policy"), Some(PERMISSIONS_POLICY));
        assert!(header(&h, "Content-Security-Policy")
            .unwrap()
            .starts_with(FIXED_DIRECTIVES));
        assert_eq!(header(&h, "Content-Disposition"), None);

        let entry = file_headers(WEBKIT, "atlas.map", "app/index.html", &[], true).unwrap();
        assert_eq!(
            header(&entry, "Content-Type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(header(&entry, "Content-Disposition"), None);

        let again = file_headers(WEBKIT, "atlas.map", "app/index.html", &[], false).unwrap();
        assert_eq!(header(&again, "Content-Disposition"), Some("attachment"));
        let unknown = file_headers(WEBKIT, "atlas.map", "app/blob.bin", &[], false).unwrap();
        assert_eq!(
            header(&unknown, "Content-Type"),
            Some("application/octet-stream")
        );
        assert_eq!(header(&unknown, "Content-Disposition"), Some("attachment"));
    }

    #[test]
    fn the_permissions_policy_turns_everything_off() {
        for item in PERMISSIONS_POLICY.split(", ") {
            assert!(item.ends_with("=()"), "{item}");
        }
        for feature in [
            "camera",
            "microphone",
            "geolocation",
            "display-capture",
            "clipboard-read",
        ] {
            assert!(
                PERMISSIONS_POLICY.contains(&format!("{feature}=()")),
                "{feature}"
            );
        }
    }

    #[test]
    fn a_refusal_is_an_empty_sandboxed_document() {
        let h = refusal_headers();
        assert!(header(&h, "Content-Security-Policy")
            .unwrap()
            .starts_with("sandbox;"));
        assert_eq!(header(&h, "X-Content-Type-Options"), Some("nosniff"));
    }
}
