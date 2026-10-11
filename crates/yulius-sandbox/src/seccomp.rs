//! The seccomp filter of contained runs (SPEC-002 ADR-22 §6), as the classic BPF program
//! `bwrap --seccomp FD` loads.
//!
//! x86-64 only. Calls of another architecture or the x32 ABI are refused with `EPERM`.
//! Refused with `EPERM`:
//! - kernel modules, kexec, `bpf`, `perf_event_open`, `userfaultfd`, keyrings, `io_uring_*`;
//! - `open_by_handle_at`, the mount APIs, `pivot_root`, `setns`;
//! - swap, reboot, clocks, `syslog`, `acct`, `quotactl`, `iopl`/`ioperm` and obsolete calls.
//!
//! Nested user namespaces: `unshare`/`clone` with `CLONE_NEWUSER` are refused, and `clone3`
//! (whose flags a filter cannot read) answers `ENOSYS`, so libc falls back to `clone`.
//! Everything else is allowed: capabilities already limit what root inside can do.

/// `AUDIT_ARCH_X86_64`.
const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
/// The x32 ABI marker in syscall numbers.
const X32_SYSCALL_BIT: u32 = 0x4000_0000;
/// `CLONE_NEWUSER`.
const CLONE_NEWUSER: u32 = 0x1000_0000;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

const LD_W_ABS: u16 = 0x20;
const JMP_JEQ_K: u16 = 0x15;
const JMP_JGE_K: u16 = 0x35;
const JMP_JSET_K: u16 = 0x45;
const RET_K: u16 = 0x06;

/// `struct seccomp_data` offsets.
const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
/// Low 32 bits of `args[0]` (little endian).
const OFFSET_ARG0: u32 = 16;

const SYS_CLONE: u32 = 56;
const SYS_UNSHARE: u32 = 272;
const SYS_CLONE3: u32 = 435;

/// Calls refused with `EPERM` (x86-64 numbers, `asm/unistd_64.h`).
pub const DENIED_X86_64: &[(u32, &str)] = &[
    (103, "syslog"),
    (134, "uselib"),
    (153, "vhangup"),
    (155, "pivot_root"),
    (156, "_sysctl"),
    (163, "acct"),
    (164, "settimeofday"),
    (165, "mount"),
    (166, "umount2"),
    (167, "swapon"),
    (168, "swapoff"),
    (169, "reboot"),
    (172, "iopl"),
    (173, "ioperm"),
    (174, "create_module"),
    (175, "init_module"),
    (176, "delete_module"),
    (177, "get_kernel_syms"),
    (178, "query_module"),
    (179, "quotactl"),
    (180, "nfsservctl"),
    (212, "lookup_dcookie"),
    (227, "clock_settime"),
    (246, "kexec_load"),
    (248, "add_key"),
    (249, "request_key"),
    (250, "keyctl"),
    (298, "perf_event_open"),
    (304, "open_by_handle_at"),
    (305, "clock_adjtime"),
    (308, "setns"),
    (313, "finit_module"),
    (320, "kexec_file_load"),
    (321, "bpf"),
    (323, "userfaultfd"),
    (425, "io_uring_setup"),
    (426, "io_uring_enter"),
    (427, "io_uring_register"),
    (428, "open_tree"),
    (429, "move_mount"),
    (430, "fsopen"),
    (431, "fsconfig"),
    (432, "fsmount"),
    (433, "fspick"),
    (442, "mount_setattr"),
];

#[derive(Clone, Copy)]
enum Target {
    Next,
    Deny,
    NoSys,
    CheckFlags,
    Allow,
}

#[derive(Clone, Copy)]
enum Op {
    Load(u32),
    Jump {
        code: u16,
        k: u32,
        yes: Target,
        no: Target,
    },
    Ret(u32),
    Label(Target),
}

/// One `struct sock_filter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insn {
    /// Opcode.
    pub code: u16,
    /// Jump offset when true.
    pub jt: u8,
    /// Jump offset when false.
    pub jf: u8,
    /// Operand.
    pub k: u32,
}

fn program() -> Vec<Op> {
    let mut ops = vec![
        Op::Load(OFFSET_ARCH),
        Op::Jump {
            code: JMP_JEQ_K,
            k: AUDIT_ARCH_X86_64,
            yes: Target::Next,
            no: Target::Deny,
        },
        Op::Load(OFFSET_NR),
        Op::Jump {
            code: JMP_JGE_K,
            k: X32_SYSCALL_BIT,
            yes: Target::Deny,
            no: Target::Next,
        },
    ];
    for (nr, _) in DENIED_X86_64 {
        ops.push(Op::Jump {
            code: JMP_JEQ_K,
            k: *nr,
            yes: Target::Deny,
            no: Target::Next,
        });
    }
    ops.extend([
        Op::Jump {
            code: JMP_JEQ_K,
            k: SYS_CLONE3,
            yes: Target::NoSys,
            no: Target::Next,
        },
        Op::Jump {
            code: JMP_JEQ_K,
            k: SYS_UNSHARE,
            yes: Target::CheckFlags,
            no: Target::Next,
        },
        Op::Jump {
            code: JMP_JEQ_K,
            k: SYS_CLONE,
            yes: Target::CheckFlags,
            no: Target::Allow,
        },
        Op::Label(Target::Allow),
        Op::Ret(RET_ALLOW),
        Op::Label(Target::CheckFlags),
        Op::Load(OFFSET_ARG0),
        Op::Jump {
            code: JMP_JSET_K,
            k: CLONE_NEWUSER,
            yes: Target::Deny,
            no: Target::Next,
        },
        Op::Ret(RET_ALLOW),
        Op::Label(Target::Deny),
        Op::Ret(RET_ERRNO | EPERM),
        Op::Label(Target::NoSys),
        Op::Ret(RET_ERRNO | ENOSYS),
    ]);
    ops
}

/// The filter as instructions, jumps resolved.
///
/// # Errors
/// A jump that does not fit the 8-bit offset (a programming error caught by the tests).
pub fn instructions() -> Result<Vec<Insn>, String> {
    let ops = program();
    // Index of each label's following instruction.
    let mut index = 0usize;
    let mut labels: Vec<(u8, usize)> = Vec::new();
    for op in &ops {
        match op {
            Op::Label(target) => labels.push((target_id(*target), index)),
            _ => index += 1,
        }
    }
    let find = |target: Target, at: usize| -> Result<u8, String> {
        let destination = match target {
            Target::Next => at + 1,
            other => labels
                .iter()
                .find(|(id, _)| *id == target_id(other))
                .map(|(_, pos)| *pos)
                .ok_or("missing label")?,
        };
        let offset = destination
            .checked_sub(at + 1)
            .ok_or("backward jump in a seccomp filter")?;
        u8::try_from(offset).map_err(|_| "seccomp jump out of range".to_owned())
    };
    let mut out = Vec::with_capacity(index);
    let mut at = 0usize;
    for op in ops {
        match op {
            Op::Label(_) => {}
            Op::Load(k) => {
                out.push(Insn {
                    code: LD_W_ABS,
                    jt: 0,
                    jf: 0,
                    k,
                });
                at += 1;
            }
            Op::Ret(k) => {
                out.push(Insn {
                    code: RET_K,
                    jt: 0,
                    jf: 0,
                    k,
                });
                at += 1;
            }
            Op::Jump { code, k, yes, no } => {
                out.push(Insn {
                    code,
                    jt: find(yes, at)?,
                    jf: find(no, at)?,
                    k,
                });
                at += 1;
            }
        }
    }
    Ok(out)
}

fn target_id(target: Target) -> u8 {
    match target {
        Target::Next => 0,
        Target::Deny => 1,
        Target::NoSys => 2,
        Target::CheckFlags => 3,
        Target::Allow => 4,
    }
}

/// The program bytes for `bwrap --seccomp` (native-endian `struct sock_filter` array), or `None`
/// on architectures without a table.
#[must_use]
pub fn program_bytes() -> Option<Vec<u8>> {
    if !cfg!(target_arch = "x86_64") {
        return None;
    }
    let insns = instructions().ok()?;
    let mut bytes = Vec::with_capacity(insns.len() * 8);
    for insn in insns {
        bytes.extend_from_slice(&insn.code.to_ne_bytes());
        bytes.push(insn.jt);
        bytes.push(insn.jf);
        bytes.extend_from_slice(&insn.k.to_ne_bytes());
    }
    Some(bytes)
}

/// Runs the filter on `(arch, nr, arg0)`, as the kernel would (for the tests).
#[must_use]
pub fn evaluate(insns: &[Insn], arch: u32, nr: u32, arg0: u32) -> Option<u32> {
    let mut accumulator = 0u32;
    let mut pc = 0usize;
    for _ in 0..insns.len() * 2 {
        let insn = insns.get(pc)?;
        match insn.code {
            LD_W_ABS => {
                accumulator = match insn.k {
                    OFFSET_NR => nr,
                    OFFSET_ARCH => arch,
                    OFFSET_ARG0 => arg0,
                    _ => return None,
                };
                pc += 1;
            }
            RET_K => return Some(insn.k),
            code => {
                let taken = match code {
                    JMP_JEQ_K => accumulator == insn.k,
                    JMP_JGE_K => accumulator >= insn.k,
                    JMP_JSET_K => accumulator & insn.k != 0,
                    _ => return None,
                };
                let offset = if taken { insn.jt } else { insn.jf };
                pc += 1 + usize::from(offset);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(nr: u32, arg0: u32) -> Option<u32> {
        let insns = instructions().ok()?;
        evaluate(&insns, AUDIT_ARCH_X86_64, nr, arg0)
    }

    #[test]
    fn ordinary_calls_are_allowed() {
        for nr in [0, 1, 2, 57, 59, 60, 101, 231, 257, 262] {
            assert_eq!(run(nr, 0), Some(RET_ALLOW), "syscall {nr}");
        }
        // fork-like clone and an unshare of a mount namespace (refused later by capabilities).
        assert_eq!(run(SYS_CLONE, 0x0120_0011), Some(RET_ALLOW));
        assert_eq!(run(SYS_UNSHARE, 0x0002_0000), Some(RET_ALLOW));
    }

    #[test]
    fn denied_calls_and_nested_user_namespaces_fail() {
        for (nr, name) in DENIED_X86_64 {
            assert_eq!(run(*nr, 0), Some(RET_ERRNO | EPERM), "{name}");
        }
        assert_eq!(run(SYS_UNSHARE, CLONE_NEWUSER), Some(RET_ERRNO | EPERM));
        assert_eq!(
            run(SYS_CLONE, CLONE_NEWUSER | 0x11),
            Some(RET_ERRNO | EPERM)
        );
        assert_eq!(run(SYS_CLONE3, 0), Some(RET_ERRNO | ENOSYS));
        assert_eq!(run(X32_SYSCALL_BIT | 1, 0), Some(RET_ERRNO | EPERM));
    }

    #[test]
    fn other_architectures_are_refused() -> Result<(), String> {
        let insns = instructions()?;
        // AUDIT_ARCH_I386
        assert_eq!(evaluate(&insns, 0x4000_0003, 1, 0), Some(RET_ERRNO | EPERM));
        Ok(())
    }

    #[test]
    fn the_program_serializes_to_eight_bytes_per_instruction() -> Result<(), String> {
        let insns = instructions()?;
        if let Some(bytes) = program_bytes() {
            assert_eq!(bytes.len(), insns.len() * 8);
        }
        assert!(insns.len() < 100);
        Ok(())
    }
}
