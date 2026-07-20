// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The classic-BPF seccomp filter for the confined Linux sidecar worker (#286 PR2d — the network half
//! of the Linux confinement; Landlock is the filesystem half). It denies, with
//! `SECCOMP_RET_ERRNO(EACCES)` (a *refused* syscall, not a killed process — catchable, and the
//! Developer-mode `net_selftest` sees the refusal):
//!
//!   * `socket(2)` for the `AF_INET` / `AF_INET6` / `AF_PACKET` families — the direct network path.
//!     `AF_UNIX` (Python multiprocessing / local IPC) and `AF_NETLINK` (glibc interface enumeration)
//!     stay allowed. The residual out-of-process DNS path (AF_UNIX → a system resolver daemon) can't be
//!     closed without breaking multiprocessing and is documented alongside the Landlock rules.
//!   * `io_uring_setup`/`_enter`/`_register` — CLOSING A REAL BYPASS: on kernel ≥ 5.19 a compromised
//!     worker could submit `IORING_OP_SOCKET`+`CONNECT`+`SEND` through an io_uring ring, creating and
//!     using an IP socket WITHOUT ever calling `socket(2)`, so a socket-only filter never sees it.
//!     Denying ring creation shuts that path; the offline worker never uses io_uring.
//!   * `ptrace` and `process_vm_readv`/`_writev` — a same-uid worker could otherwise read PM's own
//!     decrypted-vault memory out of the parent process without touching the filesystem. The worker
//!     never traces or peeks another process, so denying these costs nothing and closes that path.
//!
//! This whole module is PURE and platform-agnostic: [`build_block_inet_filter`] takes the target arch
//! as a parameter and returns plain data, and the unit tests run a hand-written cBPF interpreter over
//! BOTH the x86-64 and aarch64 variants. That is deliberate — the syscall wrapper that *installs* the
//! filter is Linux-only and can't be compiled or run on the Windows dev box, so the filter's LOGIC is
//! verified here on every platform (and in the ubuntu CI `rust` job) instead of only at runtime on a
//! real kernel. Only the raw `seccomp(2)` call in `sidecar_sandbox_linux` consumes what this produces,
//! so the whole module is dead code off Linux (the tests keep it honest meanwhile).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// One classic-BPF instruction. Field-for-field identical to the kernel's / libc's `sock_filter`
/// (`{ __u16 code; __u8 jt; __u8 jf; __u32 k; }`), so a `&[SockFilter]` can be handed straight to the
/// `seccomp(2)` syscall by reinterpreting the pointer as `*const libc::sock_filter` — the reason this
/// is a hand-rolled `repr(C)` struct rather than `libc::sock_filter` (which doesn't exist off unix, so
/// this module couldn't then be compiled/tested on Windows).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SockFilter {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

// --- BPF opcode encodings (linux/bpf_common.h), pre-OR'd (verified against kernel uapi headers). ---
const BPF_LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS — load a 32-bit word at an absolute offset
const BPF_JEQ_K: u16 = 0x15; //    BPF_JMP | BPF_JEQ | BPF_K — if A == k
const BPF_JGE_K: u16 = 0x35; //    BPF_JMP | BPF_JGE | BPF_K — if A >= k
const BPF_RET_K: u16 = 0x06; //    BPF_RET | BPF_K           — return the constant action

// --- Byte offsets into `struct seccomp_data` (native byte order). ---
const OFF_NR: u32 = 0; //       int   nr
const OFF_ARCH: u32 = 4; //     __u32 arch
const OFF_ARG0_LO: u32 = 16; // low 32 bits of args[0]  (socket domain, on little-endian)
const OFF_ARG0_HI: u32 = 20; // high 32 bits of args[0]

// --- seccomp return actions (linux/seccomp.h). ---
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const RET_ERRNO_EACCES: u32 = 0x0005_0000 | 13; // SECCOMP_RET_ERRNO | (EACCES & SECCOMP_RET_DATA)

// --- AUDIT_ARCH values (linux/audit.h) — libc does not expose these, so they're defined here. ---
const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const AUDIT_ARCH_AARCH64: u32 = 0xC000_00B7;

/// On x86-64, x32-ABI syscalls set this bit in the syscall number. A filter that checks only the
/// syscall number can be bypassed by issuing the x32 variant, so we deny anything at/above it.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

// Socket address families we deny (universal Linux values).
const AF_INET: u32 = 2;
const AF_INET6: u32 = 10;
const AF_PACKET: u32 = 17;

/// The two 64-bit Linux arches PM ships a desktop build for. Each carries the arch's `AUDIT_ARCH`
/// value, its `socket` syscall number, and whether it has an x32 compat ABI to guard against — so the
/// one [`build_block_inet_filter`] emits the correct filter for either, and the tests can drive both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SeccompArch {
    X86_64,
    Aarch64,
}

impl SeccompArch {
    /// The arch of THIS build, or `None` on a Linux arch PM doesn't ship a seccomp filter for (the
    /// caller then reports the network layer unavailable rather than shipping a wrong-arch filter).
    #[cfg(target_os = "linux")]
    pub(crate) fn current() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            Some(Self::X86_64)
        }
        #[cfg(target_arch = "aarch64")]
        {
            Some(Self::Aarch64)
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            None
        }
    }

    fn audit_arch(self) -> u32 {
        match self {
            Self::X86_64 => AUDIT_ARCH_X86_64,
            Self::Aarch64 => AUDIT_ARCH_AARCH64,
        }
    }

    /// The `socket(2)` syscall number for this arch (x86-64 = 41, aarch64 = 198).
    fn nr_socket(self) -> u32 {
        match self {
            Self::X86_64 => 41,
            Self::Aarch64 => 198,
        }
    }

    /// Syscall numbers denied outright (independent of any argument), newest-ABI network + cross-process
    /// primitives the offline worker never uses but an attacker could: `io_uring_setup`/`_enter`/
    /// `_register` (same numbers on both arches — they close the socket-less io_uring network bypass) and
    /// `ptrace` / `process_vm_readv` / `process_vm_writev` (which differ by arch — they close reading PM's
    /// decrypted-vault memory out of the parent). Hardcoded (not `libc::SYS_*`) so this stays a pure,
    /// cross-platform-testable table; the values are stable kernel ABI.
    fn always_deny(self) -> [u32; 6] {
        let (ptrace, pvm_readv, pvm_writev) = match self {
            Self::X86_64 => (101, 310, 311),
            Self::Aarch64 => (117, 270, 271),
        };
        [425, 426, 427, ptrace, pvm_readv, pvm_writev]
    }

    /// Only x86-64 has the x32 compat ABI; aarch64's only compat mode carries a different `arch`
    /// value already rejected by the arch check, so it needs no syscall-number-bit guard.
    fn has_x32(self) -> bool {
        matches!(self, Self::X86_64)
    }
}

fn stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// A forward jump distance, in the cBPF sense: `jt`/`jf` count instructions to skip AFTER the next
/// one, so reaching index `to` from index `from` skips `to - from - 1`. Panics (in the pure builder,
/// so a bug fails the unit test, never at runtime) on a backward jump or one that overflows the u8.
fn off(from: usize, to: usize) -> u8 {
    let d = to
        .checked_sub(from + 1)
        .expect("seccomp filter: forward jump only");
    u8::try_from(d).expect("seccomp filter: jump distance must fit in u8")
}

/// Build the filter for `arch`. The layout, in order:
///   1. load `arch`; if it isn't ours → KILL (blocks the x32/compat-personality bypass).
///   2. load the syscall number; on x86-64, if it has the x32 bit → KILL.
///   3. for each always-denied syscall (io_uring + ptrace/process_vm_*): if it matches → deny(EACCES).
///   4. if it isn't `socket` → ALLOW (nothing else is argument-inspected).
///   5. `socket`: load the HIGH word of `args[0]`; if non-zero → deny. `args[0]` is a 64-bit register
///      and the kernel truncates the domain to `int`, so a caller could pass `domain = (1<<32)|AF_INET`
///      and slip past a low-word-only compare — asserting the high word is zero closes that.
///   6. load the low word (the domain); AF_INET / AF_INET6 / AF_PACKET → deny(EACCES); else ALLOW.
pub(crate) fn build_block_inet_filter(arch: SeccompArch) -> Vec<SockFilter> {
    let mut f: Vec<SockFilter> = Vec::new();

    f.push(stmt(BPF_LD_W_ABS, OFF_ARCH));
    let jmp_arch = f.len();
    f.push(jump(BPF_JEQ_K, arch.audit_arch(), 0, 0)); // patched: ne -> KILL

    f.push(stmt(BPF_LD_W_ABS, OFF_NR));
    let jmp_x32 = arch.has_x32().then(|| {
        let i = f.len();
        f.push(jump(BPF_JGE_K, X32_SYSCALL_BIT, 0, 0)); // patched: ge -> KILL
        i
    });

    // Unconditionally-denied syscalls (no argument inspection): match nr → DENY, else fall through.
    let mut jmp_deny_nr = Vec::with_capacity(6);
    for nr in arch.always_deny() {
        jmp_deny_nr.push(f.len());
        f.push(jump(BPF_JEQ_K, nr, 0, 0)); // patched: eq -> DENY
    }

    let jmp_socket = f.len();
    f.push(jump(BPF_JEQ_K, arch.nr_socket(), 0, 0)); // patched: ne -> ALLOW

    f.push(stmt(BPF_LD_W_ABS, OFF_ARG0_HI));
    let jmp_hi = f.len();
    f.push(jump(BPF_JEQ_K, 0, 0, 0)); // patched: ne -> DENY (high bits set = evasion)

    f.push(stmt(BPF_LD_W_ABS, OFF_ARG0_LO));
    let mut jmp_af = Vec::with_capacity(3);
    for dom in [AF_INET, AF_INET6, AF_PACKET] {
        jmp_af.push(f.len());
        f.push(jump(BPF_JEQ_K, dom, 0, 0)); // patched: eq -> DENY
    }

    // Terminals, in fall-through order for the non-socket / allowed-domain path.
    let t_allow = f.len();
    f.push(stmt(BPF_RET_K, RET_ALLOW));
    let t_deny = f.len();
    f.push(stmt(BPF_RET_K, RET_ERRNO_EACCES));
    let t_kill = f.len();
    f.push(stmt(BPF_RET_K, RET_KILL_PROCESS));

    // Patch every jump now that the terminal positions are known.
    f[jmp_arch].jf = off(jmp_arch, t_kill);
    if let Some(i) = jmp_x32 {
        f[i].jt = off(i, t_kill);
    }
    for i in jmp_deny_nr {
        f[i].jt = off(i, t_deny);
    }
    f[jmp_socket].jf = off(jmp_socket, t_allow);
    f[jmp_hi].jf = off(jmp_hi, t_deny);
    for i in jmp_af {
        f[i].jt = off(i, t_deny);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    // Address families used only by the tests (allowed ones + a non-socket syscall nr).
    const AF_UNIX: u32 = 1;
    const AF_NETLINK: u32 = 16;

    /// Synthetic `struct seccomp_data` for the interpreter — only the fields the filter reads.
    struct Data {
        nr: u32,
        arch: u32,
        arg0: u64,
    }

    impl Data {
        /// Mirror a little-endian `BPF_LD|BPF_W|BPF_ABS` load of `seccomp_data`.
        fn load(&self, off: u32) -> u32 {
            match off {
                OFF_NR => self.nr,
                OFF_ARCH => self.arch,
                OFF_ARG0_LO => (self.arg0 & 0xffff_ffff) as u32,
                OFF_ARG0_HI => (self.arg0 >> 32) as u32,
                other => panic!("filter loaded an unexpected offset: {other}"),
            }
        }
    }

    /// A minimal classic-BPF interpreter for exactly the four opcodes the filter uses. Returns the
    /// `SECCOMP_RET_*` action the kernel would. This is what lets us PROVE the filter (which can't run
    /// on a non-Linux host, or without a real kernel) purely in a unit test.
    fn run(filter: &[SockFilter], data: &Data) -> u32 {
        let mut a: u32 = 0;
        let mut pc: usize = 0;
        for _ in 0..10_000 {
            let ins = filter[pc];
            match ins.code {
                BPF_LD_W_ABS => {
                    a = data.load(ins.k);
                    pc += 1;
                }
                BPF_JEQ_K => pc += 1 + if a == ins.k { ins.jt } else { ins.jf } as usize,
                BPF_JGE_K => pc += 1 + if a >= ins.k { ins.jt } else { ins.jf } as usize,
                BPF_RET_K => return ins.k,
                other => panic!("interpreter hit an unknown opcode {other:#x}"),
            }
        }
        panic!("filter did not terminate (no RET reached) — malformed jumps");
    }

    fn socket(domain: u32) -> u64 {
        domain as u64
    }

    /// Every arch's filter must: refuse the three IP families, allow AF_UNIX/AF_NETLINK, allow
    /// non-socket syscalls, kill a foreign arch, close the 64-bit-domain bypass, and (x86-64) kill x32.
    fn check_arch(arch: SeccompArch) {
        let f = build_block_inet_filter(arch);
        let ok_arch = arch.audit_arch();
        let sock = arch.nr_socket();

        // The three denied families -> EACCES (a refused socket, not a kill).
        for dom in [AF_INET, AF_INET6, AF_PACKET] {
            let d = Data {
                nr: sock,
                arch: ok_arch,
                arg0: socket(dom),
            };
            assert_eq!(
                run(&f, &d),
                RET_ERRNO_EACCES,
                "{arch:?}: socket(domain={dom}) should be refused with EACCES"
            );
        }

        // Allowed families pass.
        for dom in [AF_UNIX, AF_NETLINK] {
            let d = Data {
                nr: sock,
                arch: ok_arch,
                arg0: socket(dom),
            };
            assert_eq!(
                run(&f, &d),
                RET_ALLOW,
                "{arch:?}: socket(domain={dom}) should be allowed"
            );
        }

        // The 64-bit high-word bypass: domain = (1<<32) | AF_INET must still be denied.
        let bypass = Data {
            nr: sock,
            arch: ok_arch,
            arg0: (1u64 << 32) | u64::from(AF_INET),
        };
        assert_eq!(
            run(&f, &bypass),
            RET_ERRNO_EACCES,
            "{arch:?}: a high-word domain must not slip past the low-word compare"
        );

        // A non-socket, non-denied syscall (nr = 999_999) is allowed through.
        let other = Data {
            nr: 999_999,
            arch: ok_arch,
            arg0: 0,
        };
        assert_eq!(run(&f, &other), RET_ALLOW, "{arch:?}: non-socket allowed");

        // Every always-denied syscall (io_uring + ptrace/process_vm_*) is refused regardless of args.
        for nr in arch.always_deny() {
            let d = Data {
                nr,
                arch: ok_arch,
                arg0: 0,
            };
            assert_eq!(
                run(&f, &d),
                RET_ERRNO_EACCES,
                "{arch:?}: syscall {nr} should be denied"
            );
        }

        // A foreign arch is killed (blocks running the syscall under a different personality).
        let foreign = Data {
            nr: sock,
            arch: 0xdead_beef,
            arg0: socket(AF_INET),
        };
        assert_eq!(
            run(&f, &foreign),
            RET_KILL_PROCESS,
            "{arch:?}: a mismatched arch must be killed"
        );

        // x86-64 only: an x32 syscall number is killed before the socket/domain checks.
        if arch.has_x32() {
            let x32 = Data {
                nr: X32_SYSCALL_BIT | sock,
                arch: ok_arch,
                arg0: socket(AF_INET),
            };
            assert_eq!(
                run(&f, &x32),
                RET_KILL_PROCESS,
                "{arch:?}: an x32 syscall must be killed"
            );
        }
    }

    #[test]
    fn x86_64_filter_blocks_ip_allows_unix_and_kills_x32() {
        check_arch(SeccompArch::X86_64);
        // Length: 11 body + 6 always-deny + 3 terminals (the x32 guard is present on x86-64).
        assert_eq!(build_block_inet_filter(SeccompArch::X86_64).len(), 20);
    }

    #[test]
    fn aarch64_filter_blocks_ip_allows_unix_no_x32_guard() {
        check_arch(SeccompArch::Aarch64);
        // Length: 10 body + 6 always-deny + 3 terminals (no x32 guard on aarch64).
        assert_eq!(build_block_inet_filter(SeccompArch::Aarch64).len(), 19);
    }

    /// The last three instructions are always the ALLOW / DENY / KILL terminals, in that order.
    #[test]
    fn terminals_are_the_three_return_actions() {
        for arch in [SeccompArch::X86_64, SeccompArch::Aarch64] {
            let f = build_block_inet_filter(arch);
            let n = f.len();
            assert_eq!(f[n - 3], stmt(BPF_RET_K, RET_ALLOW), "{arch:?} allow");
            assert_eq!(f[n - 2], stmt(BPF_RET_K, RET_ERRNO_EACCES), "{arch:?} deny");
            assert_eq!(f[n - 1], stmt(BPF_RET_K, RET_KILL_PROCESS), "{arch:?} kill");
        }
    }
}
