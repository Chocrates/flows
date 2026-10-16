//! seccomp filter applied inside the bubblewrap sandbox (via `bwrap --seccomp`).
//!
//! The filesystem bind is read-only, but a read-only mount does not stop
//! `connect()` on a unix socket. Without this filter a sandboxed command could
//! talk to the user's systemd/D-Bus session bus (`systemd-run --user …` runs
//! arbitrary commands *outside* the sandbox), a container engine socket, the
//! display server, or any abstract-namespace socket (shared because the network
//! namespace is shared). The filter closes all of those at once:
//!
//! * `socket(AF_UNIX, …)` → `EPERM`. `socketpair()` stays allowed, so ordinary
//!   in-process IPC (asyncio, multiprocessing, libuv) keeps working.
//! * `io_uring_setup` → `EPERM` — io_uring can create sockets without going
//!   through the `socket` syscall, which seccomp would not see.
//! * Foreign-ABI syscalls (a non-native audit arch, or the x32 ABI on x86_64) →
//!   `EPERM`, so the rules can't be sidestepped by switching syscall tables.
//!
//! Classic BPF, hand-assembled: no libseccomp dependency.

/// One BPF instruction (`struct sock_filter`).
#[derive(Clone, Copy)]
struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const JGE_K: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
const RET_K: u16 = 0x06; // BPF_RET | BPF_K

const RET_ALLOW: u32 = 0x7fff_0000;
const RET_EPERM: u32 = 0x0005_0000 | 1; // SECCOMP_RET_ERRNO | EPERM

// `struct seccomp_data` offsets.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
const OFF_ARG0: u32 = 16; // low 32 bits on little-endian

const AF_UNIX: u32 = 1;

#[cfg(target_arch = "x86_64")]
mod arch {
    pub const AUDIT_ARCH: u32 = 0xC000_003E;
    pub const NR_SOCKET: u32 = 41;
    pub const NR_IO_URING_SETUP: u32 = 425;
    /// x32 syscalls set this bit; deny them all.
    pub const X32_BIT: Option<u32> = Some(0x4000_0000);
}

#[cfg(target_arch = "aarch64")]
mod arch {
    pub const AUDIT_ARCH: u32 = 0xC000_00B7;
    pub const NR_SOCKET: u32 = 198;
    pub const NR_IO_URING_SETUP: u32 = 425;
    pub const X32_BIT: Option<u32> = None;
}

/// True when this build knows the syscall numbers for its architecture.
pub const SUPPORTED: bool = cfg!(any(target_arch = "x86_64", target_arch = "aarch64"));

/// The filter as raw `sock_filter` bytes, ready for `bwrap --seccomp FD`.
/// `None` on architectures without a syscall table here.
pub fn program() -> Option<Vec<u8>> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        Some(encode(&build(
            arch::AUDIT_ARCH,
            arch::NR_SOCKET,
            arch::NR_IO_URING_SETUP,
            arch::X32_BIT,
        )))
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        None
    }
}

/// Jump targets resolved after assembly.
#[derive(Clone, Copy)]
enum To {
    Next,
    Deny,
    CheckArg,
}

fn build(audit_arch: u32, nr_socket: u32, nr_io_uring: u32, x32_bit: Option<u32>) -> Vec<Insn> {
    // (code, jt, jf, k) with symbolic targets; `Next` = fall through.
    let mut prog: Vec<(u16, To, To, u32)> = vec![
        (LD_W_ABS, To::Next, To::Next, OFF_ARCH),
        (JEQ_K, To::Next, To::Deny, audit_arch),
        (LD_W_ABS, To::Next, To::Next, OFF_NR),
    ];
    if let Some(bit) = x32_bit {
        prog.push((JGE_K, To::Deny, To::Next, bit));
    }
    prog.extend([
        (JEQ_K, To::CheckArg, To::Next, nr_socket),
        (JEQ_K, To::Deny, To::Next, nr_io_uring),
        (RET_K, To::Next, To::Next, RET_ALLOW),
    ]);
    let check_arg = prog.len();
    prog.extend([
        (LD_W_ABS, To::Next, To::Next, OFF_ARG0),
        (JEQ_K, To::Deny, To::Next, AF_UNIX),
        (RET_K, To::Next, To::Next, RET_ALLOW),
    ]);
    let deny = prog.len();
    prog.push((RET_K, To::Next, To::Next, RET_EPERM));

    let rel = |i: usize, t: To| -> u8 {
        let target = match t {
            To::Next => return 0,
            To::Deny => deny,
            To::CheckArg => check_arg,
        };
        (target - i - 1) as u8
    };
    prog.iter()
        .enumerate()
        .map(|(i, &(code, jt, jf, k))| Insn {
            code,
            jt: rel(i, jt),
            jf: rel(i, jf),
            k,
        })
        .collect()
}

fn encode(prog: &[Insn]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prog.len() * 8);
    for i in prog {
        out.extend_from_slice(&i.code.to_ne_bytes());
        out.push(i.jt);
        out.push(i.jf);
        out.extend_from_slice(&i.k.to_ne_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal classic-BPF interpreter over `seccomp_data` for the ops we emit.
    fn run(prog: &[Insn], arch: u32, nr: u32, arg0: u32) -> u32 {
        let mut pc = 0;
        let mut acc = 0u32;
        loop {
            let i = prog[pc];
            match i.code {
                LD_W_ABS => {
                    acc = match i.k {
                        OFF_NR => nr,
                        OFF_ARCH => arch,
                        OFF_ARG0 => arg0,
                        other => panic!("unexpected offset {other}"),
                    };
                    pc += 1;
                }
                JEQ_K => pc += 1 + if acc == i.k { i.jt } else { i.jf } as usize,
                JGE_K => pc += 1 + if acc >= i.k { i.jt } else { i.jf } as usize,
                RET_K => return i.k,
                other => panic!("unexpected opcode {other:#x}"),
            }
        }
    }

    const A: u32 = 0xC000_003E;
    fn x86() -> Vec<Insn> {
        build(A, 41, 425, Some(0x4000_0000))
    }

    #[test]
    fn unix_socket_denied() {
        assert_eq!(run(&x86(), A, 41, AF_UNIX), RET_EPERM);
    }

    #[test]
    fn inet_sockets_allowed() {
        assert_eq!(run(&x86(), A, 41, 2), RET_ALLOW); // AF_INET
        assert_eq!(run(&x86(), A, 41, 10), RET_ALLOW); // AF_INET6
    }

    #[test]
    fn io_uring_denied_other_syscalls_allowed() {
        assert_eq!(run(&x86(), A, 425, 0), RET_EPERM);
        assert_eq!(run(&x86(), A, 0, 0), RET_ALLOW); // read
        assert_eq!(run(&x86(), A, 53, AF_UNIX), RET_ALLOW); // socketpair(AF_UNIX)
    }

    #[test]
    fn foreign_arch_and_x32_denied() {
        assert_eq!(run(&x86(), 0x4000_0003, 41, 2), RET_EPERM); // i386
        assert_eq!(run(&x86(), A, 0x4000_0000 | 41, 2), RET_EPERM); // x32 socket
    }

    #[test]
    fn aarch64_layout_has_no_x32_check() {
        let p = build(0xC000_00B7, 198, 425, None);
        assert_eq!(run(&p, 0xC000_00B7, 198, AF_UNIX), RET_EPERM);
        assert_eq!(run(&p, 0xC000_00B7, 198, 2), RET_ALLOW);
        assert_eq!(run(&p, 0xC000_00B7, 0x4000_0000, 0), RET_ALLOW);
    }

    #[test]
    fn encoding_is_8_bytes_per_insn() {
        let p = x86();
        assert_eq!(encode(&p).len(), p.len() * 8);
        if SUPPORTED {
            assert!(program().is_some());
        }
    }
}
