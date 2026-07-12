use super::*;
use object::{Object, ObjectSymbol};
use std::ffi::c_void;
use std::sync::OnceLock;

pub struct TemplateJit;

// Copy-and-patch: `compile` builds the exact same register-allocated `Op` IR as
// `bytecode_register_become` (locals get fixed registers by nesting depth, args are
// preloaded into low registers, temporaries bump-allocate, `Set`/`Add` fold literals
// directly into their destination register), but instead of encoding that IR into a
// custom bytecode format, it "links" it directly into native machine code: for each
// `Op`, the real, `rustc`-compiled machine code of a matching template function below
// is copied into a fresh executable buffer and its "holes" (register indices,
// immediates, successor addresses) are patched with this instruction's concrete
// operands. The result is a self-contained native function — `execute` just calls it.
enum Op {
    LoadImm(usize, i64),
    Move(usize, usize),
    Add(usize, usize, usize),
    AddImm(usize, usize, i64),
    JmpZN(usize, usize),
    Jmp(usize),
    Ret(usize),
}

pub struct Program {
    base: *mut u8,
    len: usize,
    num_regs: usize,
    n_args: usize,
    // (byte offset, human-readable description) per op, for disassembly/inspection.
    labels: Vec<(usize, String)>,
}

impl Program {
    /// The compiled native code, for disassembly/inspection.
    pub fn code(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.base, self.len) }
    }

    /// (byte offset, description) for each compiled instruction's start, in order.
    pub fn labels(&self) -> &[(usize, String)] {
        &self.labels
    }
}

// SAFETY: `base` points at a private JIT-owned executable mapping; nothing else
// aliases it, so it's fine to hand `&Program`/`Program` across threads.
unsafe impl Send for Program {}
unsafe impl Sync for Program {}

impl Drop for Program {
    fn drop(&mut self) {
        unsafe {
            munmap(self.base.cast(), self.len);
        }
    }
}

impl Vm for TemplateJit {
    type Program<'a> = Program;

    fn compile(expr: &Expr) -> Self::Program<'_> {
        struct Ctx {
            ops: Vec<Op>,
            max_reg: usize,
        }

        impl Ctx {
            fn touch(&mut self, reg: usize) {
                self.max_reg = self.max_reg.max(reg + 1);
            }
        }

        fn argmax(expr: &Expr) -> usize {
            match expr {
                Expr::Litr(_) | Expr::Get(_) => 0,
                Expr::Arg(i) => i + 1,
                Expr::Add(x, y) | Expr::Let(x, y) | Expr::While(x, y) | Expr::Then(x, y) => {
                    argmax(x).max(argmax(y))
                }
                Expr::Set(_, x) => argmax(x),
            }
        }

        fn compile_inner(ctx: &mut Ctx, expr: &Expr, depth: usize, next_reg: usize) -> (usize, bool) {
            match expr {
                Expr::Litr(x) => {
                    ctx.ops.push(Op::LoadImm(next_reg, *x));
                    ctx.touch(next_reg);
                    (next_reg, true)
                }
                Expr::Arg(idx) => (*idx, false),
                Expr::Get(local) => (depth - 1 - local, false),
                Expr::Add(x, y) => {
                    let dest = next_reg;
                    if let Expr::Litr(imm) = &**y {
                        let (rx, _) = compile_inner(ctx, x, depth, next_reg);
                        ctx.ops.push(Op::AddImm(dest, rx, *imm));
                    } else {
                        let (rx, x_temp) = compile_inner(ctx, x, depth, next_reg);
                        let y_reg = if x_temp { rx + 1 } else { next_reg };
                        let (ry, _) = compile_inner(ctx, y, depth, y_reg);
                        ctx.ops.push(Op::Add(dest, rx, ry));
                    }
                    ctx.touch(dest);
                    (dest, true)
                }
                Expr::Let(rhs, then) => {
                    let (r_rhs, _) = compile_inner(ctx, rhs, depth, depth);
                    if r_rhs != depth {
                        ctx.ops.push(Op::Move(depth, r_rhs));
                    }
                    ctx.touch(depth);
                    compile_inner(ctx, then, depth + 1, depth + 1)
                }
                Expr::Set(local, rhs) => {
                    let target = depth - 1 - local;
                    match &**rhs {
                        Expr::Litr(x) => {
                            ctx.ops.push(Op::LoadImm(target, *x));
                            ctx.touch(target);
                        }
                        Expr::Add(x, y) => {
                            if let Expr::Litr(imm) = &**y {
                                let (rx, _) = compile_inner(ctx, x, depth, depth);
                                ctx.ops.push(Op::AddImm(target, rx, *imm));
                            } else {
                                let (rx, x_temp) = compile_inner(ctx, x, depth, depth);
                                let y_reg = if x_temp { rx + 1 } else { depth };
                                let (ry, _) = compile_inner(ctx, y, depth, y_reg);
                                ctx.ops.push(Op::Add(target, rx, ry));
                            }
                            ctx.touch(target);
                        }
                        _ => {
                            let (r_rhs, _) = compile_inner(ctx, rhs, depth, depth);
                            if r_rhs != target {
                                ctx.ops.push(Op::Move(target, r_rhs));
                            }
                        }
                    }
                    (target, false)
                }
                Expr::While(pred, body) => {
                    let start = ctx.ops.len();
                    let (r_pred, _) = compile_inner(ctx, pred, depth, depth);
                    let branch_fixup = ctx.ops.len();
                    ctx.ops.push(Op::JmpZN(r_pred, 0));
                    compile_inner(ctx, body, depth, depth);
                    ctx.ops.push(Op::Jmp(start));
                    let end = ctx.ops.len();
                    ctx.ops[branch_fixup] = Op::JmpZN(r_pred, end);
                    (depth, false)
                }
                Expr::Then(a, b) => {
                    compile_inner(ctx, a, depth, depth);
                    compile_inner(ctx, b, depth, depth)
                }
            }
        }

        let n_args = argmax(expr);

        let mut ctx = Ctx {
            ops: Vec::new(),
            max_reg: 0,
        };

        let (result, _) = compile_inner(&mut ctx, expr, n_args, n_args);
        ctx.ops.push(Op::Ret(result));

        link(&ctx.ops, ctx.max_reg.max(n_args).max(1), n_args)
    }

    unsafe fn execute(prog: &Self::Program<'_>, args: &[i64]) -> i64 {
        let mut regs = vec![0i64; prog.num_regs];
        debug_assert!(args.len() >= prog.n_args, "not enough arguments");
        regs[..prog.n_args].copy_from_slice(&args[..prog.n_args]);
        let entry: unsafe extern "rust-preserve-none" fn(*mut i64) -> i64 =
            unsafe { std::mem::transmute(prog.base) };
        unsafe { entry(regs.as_mut_ptr()) }
    }
}

fn link(ops: &[Op], num_regs: usize, n_args: usize) -> Program {
    let t = templates();

    let op_len = |op: &Op| -> usize {
        match op {
            Op::LoadImm(..) => t.loadimm.base.len,
            Op::Move(..) => t.mov.base.len,
            Op::Add(..) => t.add.base.len,
            Op::AddImm(..) => t.addimm.base.len,
            Op::JmpZN(..) => t.jmpzn.base.len,
            Op::Jmp(..) => t.jmp.base.len,
            Op::Ret(..) => t.ret.base.len,
        }
    };

    let mut offset = Vec::with_capacity(ops.len());
    let mut cursor = 0usize;
    for op in ops {
        offset.push(cursor);
        cursor += op_len(op);
    }
    let total = cursor;

    let map = unsafe {
        mmap(
            std::ptr::null_mut(),
            total,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        )
    };
    assert!(!map.is_null() && map as isize != -1, "mmap failed: {}", std::io::Error::last_os_error());
    let base = map.cast::<u8>();
    let buf = unsafe { std::slice::from_raw_parts_mut(base, total) };

    let addr_of = |target: usize| -> usize { base as usize + offset[target] };

    let labels: Vec<(usize, String)> = ops
        .iter()
        .enumerate()
        .map(|(i, op)| {
            let desc = match *op {
                Op::LoadImm(dest, imm) => format!("i{i:03}_loadimm_r{dest}_{imm}"),
                Op::Move(dest, src) => format!("i{i:03}_move_r{dest}_r{src}"),
                Op::Add(dest, a, b) => format!("i{i:03}_add_r{dest}_r{a}_r{b}"),
                Op::AddImm(dest, a, imm) => format!("i{i:03}_addimm_r{dest}_r{a}_{imm}"),
                Op::JmpZN(reg, target) => format!("i{i:03}_jmpzn_r{reg}_to_i{target:03}"),
                Op::Jmp(target) => format!("i{i:03}_jmp_to_i{target:03}"),
                Op::Ret(reg) => format!("i{i:03}_ret_r{reg}"),
            };
            (offset[i], desc)
        })
        .collect();

    for (i, op) in ops.iter().enumerate() {
        let start = offset[i];
        let len = op_len(op);
        let dst = &mut buf[start..start + len];
        let dst_addr = base as usize + start;
        match *op {
            Op::LoadImm(dest, imm) => {
                dst.copy_from_slice(t.loadimm.base.bytes());
                apply_value_hole(dst, t.loadimm.dest, dest);
                apply_value_hole(dst, t.loadimm.imm, imm as usize);
                apply_branch_hole(dst, t.loadimm.next, dst_addr, addr_of(i + 1));
            }
            Op::Move(dest, src) => {
                dst.copy_from_slice(t.mov.base.bytes());
                apply_value_hole(dst, t.mov.dest, dest);
                apply_value_hole(dst, t.mov.src, src);
                apply_branch_hole(dst, t.mov.next, dst_addr, addr_of(i + 1));
            }
            Op::Add(dest, a, b) => {
                dst.copy_from_slice(t.add.base.bytes());
                apply_value_hole(dst, t.add.dest, dest);
                apply_value_hole(dst, t.add.a, a);
                apply_value_hole(dst, t.add.b, b);
                apply_branch_hole(dst, t.add.next, dst_addr, addr_of(i + 1));
            }
            Op::AddImm(dest, a, imm) => {
                dst.copy_from_slice(t.addimm.base.bytes());
                apply_value_hole(dst, t.addimm.dest, dest);
                apply_value_hole(dst, t.addimm.a, a);
                apply_value_hole(dst, t.addimm.imm, imm as usize);
                apply_branch_hole(dst, t.addimm.next, dst_addr, addr_of(i + 1));
            }
            Op::JmpZN(reg, target) => {
                dst.copy_from_slice(t.jmpzn.base.bytes());
                apply_value_hole(dst, t.jmpzn.reg, reg);
                apply_branch_hole(dst, t.jmpzn.taken, dst_addr, addr_of(target));
                apply_branch_hole(dst, t.jmpzn.fallthrough, dst_addr, addr_of(i + 1));
            }
            Op::Jmp(target) => {
                dst.copy_from_slice(t.jmp.base.bytes());
                apply_branch_hole(dst, t.jmp.target, dst_addr, addr_of(target));
            }
            Op::Ret(reg) => {
                dst.copy_from_slice(t.ret.base.bytes());
                apply_value_hole(dst, t.ret.reg, reg);
            }
        }
    }

    let rc = unsafe { mprotect(map, total, PROT_READ | PROT_EXEC) };
    assert_eq!(rc, 0, "mprotect failed: {}", std::io::Error::last_os_error());
    unsafe {
        sys_icache_invalidate(map, total);
    }

    Program {
        base,
        len: total,
        num_regs,
        n_args,
        labels,
    }
}

// Register-index/immediate holes are 64-bit sentinel constants, chosen so every
// 16-bit chunk is non-zero (so LLVM always materializes them as 4 back-to-back
// `movz`+`movk` instructions rather than skipping a zero chunk — see `chunks_ok`). We
// locate each one's 4-instruction sequence once per template at startup
// (`find_value_hole`) and thereafter just overwrite the cached offset's 4 immediate
// fields directly.
const HOLE_1: usize = 0x1111_2222_3333_4445;
const HOLE_2: usize = 0x5555_6666_7777_8889;
const HOLE_3: usize = 0x9999_aaaa_bbbb_cccd;
const HOLE_IMM: usize = 0xdddd_eeee_cdef_1235;

const fn chunks_ok(v: usize) -> bool {
    let mut k = 0;
    while k < 4 {
        let chunk = (v >> (k * 16)) & 0xFFFF;
        if chunk == 0 || chunk == 0xFFFF {
            return false;
        }
        k += 1;
    }
    true
}
const _: () = assert!(chunks_ok(HOLE_1));
const _: () = assert!(chunks_ok(HOLE_2));
const _: () = assert!(chunks_ok(HOLE_3));
const _: () = assert!(chunks_ok(HOLE_IMM));

const fn hole_chunk(v: usize, k: usize) -> u16 {
    (v >> (k * 16)) as u16
}

// Materializes a hole sentinel into a register via a hand-written `movz`+`movk`
// sequence instead of `std::hint::black_box(CONST)`. `black_box` works (it also
// prevents the optimizer from const-folding the value away) but implements its
// optimization barrier as a memory round-trip, forcing every hole through a spill to
// the stack and back — real overhead on every single instruction of the compiled
// program. Hand-written `asm!` gets the same "opaque to the optimizer" property for
// free (inline asm is never analyzed across its boundary) while letting the value
// stay in whatever register the allocator already picked, with `options(nomem,
// nostack, preserves_flags)` telling the compiler it's safe to skip even the usual
// conservative frame setup around an asm block.
macro_rules! load_hole {
    ($sentinel:expr) => {{
        let value: usize;
        std::arch::asm!(
            "movz {0}, #{c0}",
            "movk {0}, #{c1}, lsl #16",
            "movk {0}, #{c2}, lsl #32",
            "movk {0}, #{c3}, lsl #48",
            out(reg) value,
            c0 = const hole_chunk($sentinel, 0),
            c1 = const hole_chunk($sentinel, 1),
            c2 = const hole_chunk($sentinel, 2),
            c3 = const hole_chunk($sentinel, 3),
            options(nomem, nostack, preserves_flags),
        );
        value
    }};
}

fn find_value_hole(bytes: &[u8], sentinel: usize) -> usize {
    let chunks: [u16; 4] = std::array::from_fn(|k| (sentinel >> (k * 16)) as u16);
    for i in (0..bytes.len().saturating_sub(15)).step_by(4) {
        let matches = (0..4).all(|k| {
            let word = u32::from_le_bytes(bytes[i + k * 4..i + k * 4 + 4].try_into().unwrap());
            let is_movz_or_movk = word & 0x7F80_0000 == 0x5280_0000 || word & 0x7F80_0000 == 0x7280_0000;
            is_movz_or_movk && ((word >> 5) & 0xFFFF) as u16 == chunks[k]
        });
        if matches {
            return i;
        }
    }
    panic!("value hole not found for sentinel {sentinel:#x}");
}

// AArch64 `nop`: `0xD503201F`.
const NOP: u32 = 0xD503_201F;

// The template is always compiled as `movz`+`movk`x3, sized for a worst-case 64-bit
// value — but most hole values in practice are small (register indices) or small
// negative numbers (`Litr(-1)` and friends), meaning most of the four 16-bit chunks
// are either all-zero or all-one. `movz` implicitly zero-fills every bit its `movk`s
// don't touch, and `movn` (same encoding, differing only in bit 30 — verified against
// a hand-assembled `movz`/`movn` pair) implicitly one-fills instead. So: pick
// whichever base instruction lets more of the three `movk`s collapse into a `nop`
// (same instruction count, but no register dependency and no execution port cost —
// `movk #0x0,...` and an actual `nop` are not the same thing to the CPU): a chunk
// that already matches the base's implicit fill value needs no `movk` at all.
fn apply_value_hole(buf: &mut [u8], offset: usize, value: usize) {
    let chunks: [u16; 4] = std::array::from_fn(|k| (value >> (k * 16)) as u16);

    let zero_skippable = chunks[1..].iter().filter(|&&c| c == 0x0000).count();
    let ones_skippable = chunks[1..].iter().filter(|&&c| c == 0xFFFF).count();
    let use_movn = ones_skippable > zero_skippable;

    // `movn Rd, #imm16` computes `NOT(zero_extend(imm16))`, so to land on `chunks[0]`
    // in the low 16 bits when using it as the base, the encoded immediate has to be
    // its complement.
    let base_word = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
    let base_imm = if use_movn { !chunks[0] } else { chunks[0] };
    let mut new_base = (base_word & !(0xFFFFu32 << 5)) | ((base_imm as u32) << 5);
    if use_movn {
        new_base &= !(1 << 30); // movz -> movn: clear opc's high bit
    }
    buf[offset..offset + 4].copy_from_slice(&new_base.to_le_bytes());

    let skip_value = if use_movn { 0xFFFFu16 } else { 0x0000u16 };
    for k in 1..4 {
        let off = offset + k * 4;
        if chunks[k] == skip_value {
            buf[off..off + 4].copy_from_slice(&NOP.to_le_bytes());
        } else {
            let word = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
            let patched = (word & !(0xFFFFu32 << 5)) | ((chunks[k] as u32) << 5);
            buf[off..off + 4].copy_from_slice(&patched.to_le_bytes());
        }
    }
}

// Successor ("next"/target/fallthrough) holes are `become`-tail-calls to a real,
// `#[inline(never)]` marker function (`tj_marker`/`tj_marker2`, never actually meant
// to run — every reference to them gets patched before the code is ever executed).
// Tail-calling a real, non-trivial, non-inlinable function compiles to a single
// direct AArch64 `B` instruction carrying its own 26-bit PC-relative offset — no
// register has to be loaded with an absolute address first, unlike the address-in-a-
// sentinel-then-indirect-`br` approach this replaced, which cost 5 extra instructions
// (4 to build the 64-bit address, 1 indirect branch) on every single instruction of
// the compiled program, including straight-line ones with nothing to branch around.
fn find_branch_hole(bytes: &[u8], template_live_addr: usize, marker_live_addr: usize) -> usize {
    for i in (0..bytes.len()).step_by(4) {
        let word = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        // Unconditional B: bits[31:26] = 000101, imm26 = bits[25:0], a signed
        // word-offset (i.e. `*4` bytes) relative to this instruction's own address.
        if word & 0xFC00_0000 != 0x1400_0000 {
            continue;
        }
        let imm26 = (word & 0x03FF_FFFF) as i32;
        let simm26 = (imm26 << 6) >> 6; // sign-extend 26 -> 32 bits
        let branch_addr = template_live_addr + i;
        let target = (branch_addr as i64 + (simm26 as i64) * 4) as usize;
        if target == marker_live_addr {
            return i;
        }
    }
    panic!("branch hole not found for marker {marker_live_addr:#x}");
}

fn apply_branch_hole(buf: &mut [u8], offset: usize, op_base_addr: usize, target_addr: usize) {
    let branch_addr = op_base_addr + offset;
    let delta = target_addr as i64 - branch_addr as i64;
    debug_assert!(delta % 4 == 0, "branch target not instruction-aligned");
    let word_offset = delta / 4;
    debug_assert!((-(1i64 << 25)..(1i64 << 25)).contains(&word_offset), "branch out of AArch64's ±128MB range");
    let encoded = 0x1400_0000u32 | (word_offset as u32 & 0x03FF_FFFF);
    buf[offset..offset + 4].copy_from_slice(&encoded.to_le_bytes());
}

struct TemplateInfo {
    ptr: *const u8,
    len: usize,
}

impl TemplateInfo {
    fn bytes(&self) -> &'static [u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

struct LoadImmTpl {
    base: TemplateInfo,
    dest: usize,
    imm: usize,
    next: usize,
}
struct MoveTpl {
    base: TemplateInfo,
    dest: usize,
    src: usize,
    next: usize,
}
struct AddTpl {
    base: TemplateInfo,
    dest: usize,
    a: usize,
    b: usize,
    next: usize,
}
struct AddImmTpl {
    base: TemplateInfo,
    dest: usize,
    a: usize,
    imm: usize,
    next: usize,
}
struct JmpZnTpl {
    base: TemplateInfo,
    reg: usize,
    taken: usize,
    fallthrough: usize,
}
struct JmpTpl {
    base: TemplateInfo,
    target: usize,
}
struct RetTpl {
    base: TemplateInfo,
    reg: usize,
}

struct Templates {
    loadimm: LoadImmTpl,
    mov: MoveTpl,
    add: AddTpl,
    addimm: AddImmTpl,
    jmpzn: JmpZnTpl,
    jmp: JmpTpl,
    ret: RetTpl,
}

// SAFETY: template code lives in the binary's read-only `__TEXT` segment for the
// whole process lifetime.
unsafe impl Send for Templates {}
unsafe impl Sync for Templates {}

// `#[unsafe(no_mangle)]` alone only stops the *name* from being mangled — a linker
// producing a final binary still garbage-collects any of these functions that isn't
// reachable from a real Rust call/address-of expression, since our only other uses of
// them are through `symbol_bounds`' string-keyed lookup below, which the linker can't
// see. Taking their addresses here as ordinary function pointers is what keeps all of
// them in the binary.
#[used]
static KEEP_ALIVE: [unsafe extern "rust-preserve-none" fn(*mut i64) -> i64; 9] = [
    tj_loadimm,
    tj_move,
    tj_add,
    tj_addimm,
    tj_jmpzn,
    tj_jmp,
    tj_ret,
    tj_marker,
    tj_marker2,
];

fn templates() -> &'static Templates {
    static TEMPLATES: OnceLock<Templates> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let names = [
            "tj_loadimm",
            "tj_move",
            "tj_add",
            "tj_addimm",
            "tj_jmpzn",
            "tj_jmp",
            "tj_ret",
            "tj_marker",
            "tj_marker2",
        ];
        let bounds = symbol_bounds(&names);
        // Every one of our template symbols lives in the currently-running image, so
        // any single one gives us the ASLR slide between the file's linked addresses
        // and where this process actually loaded `__TEXT`.
        let slide = std::hint::black_box(KEEP_ALIVE[0]) as *const () as usize - bounds["tj_loadimm"].0;
        let info = |name: &str| {
            let (file_addr, len) = bounds[name];
            TemplateInfo {
                ptr: (file_addr + slide) as *const u8,
                len,
            }
        };

        let marker_addr = bounds["tj_marker"].0 + slide;
        let marker2_addr = bounds["tj_marker2"].0 + slide;

        let loadimm = info("tj_loadimm");
        let loadimm_bytes = loadimm.bytes();
        let loadimm_next = find_branch_hole(loadimm_bytes, loadimm.ptr as usize, marker_addr);
        let loadimm = LoadImmTpl {
            dest: find_value_hole(loadimm_bytes, HOLE_1),
            imm: find_value_hole(loadimm_bytes, HOLE_IMM),
            next: loadimm_next,
            base: loadimm,
        };

        let mov = info("tj_move");
        let mov_bytes = mov.bytes();
        let mov_next = find_branch_hole(mov_bytes, mov.ptr as usize, marker_addr);
        let mov = MoveTpl {
            dest: find_value_hole(mov_bytes, HOLE_1),
            src: find_value_hole(mov_bytes, HOLE_2),
            next: mov_next,
            base: mov,
        };

        let add = info("tj_add");
        let add_bytes = add.bytes();
        let add_next = find_branch_hole(add_bytes, add.ptr as usize, marker_addr);
        let add = AddTpl {
            dest: find_value_hole(add_bytes, HOLE_1),
            a: find_value_hole(add_bytes, HOLE_2),
            b: find_value_hole(add_bytes, HOLE_3),
            next: add_next,
            base: add,
        };

        let addimm = info("tj_addimm");
        let addimm_bytes = addimm.bytes();
        let addimm_next = find_branch_hole(addimm_bytes, addimm.ptr as usize, marker_addr);
        let addimm = AddImmTpl {
            dest: find_value_hole(addimm_bytes, HOLE_1),
            a: find_value_hole(addimm_bytes, HOLE_2),
            imm: find_value_hole(addimm_bytes, HOLE_IMM),
            next: addimm_next,
            base: addimm,
        };

        let jmpzn = info("tj_jmpzn");
        let jmpzn_bytes = jmpzn.bytes();
        let jmpzn_taken = find_branch_hole(jmpzn_bytes, jmpzn.ptr as usize, marker_addr);
        let jmpzn_fallthrough = find_branch_hole(jmpzn_bytes, jmpzn.ptr as usize, marker2_addr);
        let jmpzn = JmpZnTpl {
            reg: find_value_hole(jmpzn_bytes, HOLE_1),
            taken: jmpzn_taken,
            fallthrough: jmpzn_fallthrough,
            base: jmpzn,
        };

        let jmp = info("tj_jmp");
        let jmp_bytes = jmp.bytes();
        let jmp_target = find_branch_hole(jmp_bytes, jmp.ptr as usize, marker_addr);
        let jmp = JmpTpl {
            target: jmp_target,
            base: jmp,
        };

        let ret = info("tj_ret");
        let ret_bytes = ret.bytes();
        let ret = RetTpl {
            reg: find_value_hole(ret_bytes, HOLE_1),
            base: ret,
        };

        Templates {
            loadimm,
            mov,
            add,
            addimm,
            jmpzn,
            jmp,
            ret,
        }
    })
}

// Reads this process's own executable off disk and, for each requested `#[no_mangle]`
// symbol, returns its (file-linked address, length) — length is the distance to the
// next symbol in address order, since Mach-O's symbol table (unlike ELF's) carries no
// size field.
fn symbol_bounds(names: &[&str]) -> std::collections::HashMap<String, (usize, usize)> {
    let exe = std::env::current_exe().expect("current_exe");
    let data = std::fs::read(&exe).expect("read current_exe");
    let obj = object::File::parse(&*data).expect("parse current_exe");

    let mut syms: Vec<(String, usize)> = obj
        .symbols()
        .filter_map(|s| s.name().ok().map(|n| (n.to_string(), s.address() as usize)))
        .filter(|(_, addr)| *addr != 0)
        .collect();
    syms.sort_by_key(|(_, addr)| *addr);

    let mut out = std::collections::HashMap::new();
    for name in names {
        let mangled = format!("_{name}");
        let idx = syms
            .iter()
            .position(|(n, _)| n == &mangled || n == name)
            .unwrap_or_else(|| panic!("symbol not found: {name}"));
        let addr = syms[idx].1;
        let end = syms[idx + 1..]
            .iter()
            .find(|(_, a)| *a > addr)
            .map(|(_, a)| *a)
            .unwrap_or(addr);
        out.insert((*name).to_string(), (addr, end - addr));
    }
    out
}

unsafe extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut c_void;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn sys_icache_invalidate(start: *mut c_void, len: usize);
}

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const PROT_EXEC: i32 = 4;
const MAP_PRIVATE: i32 = 0x0002;
const MAP_ANON: i32 = 0x1000;

// Never actually executed — every `become tj_marker(...)`/`become tj_marker2(...)` in
// the templates below gets patched into a branch to somewhere else before the JIT'd
// code ever runs. They just need to be real, separate, non-inlinable functions so the
// templates compile to genuine (and individually identifiable) direct branches.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_marker(regs: *mut i64) -> i64 {
    unsafe { std::hint::black_box(*regs) }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_marker2(regs: *mut i64) -> i64 {
    unsafe { std::hint::black_box(*regs.add(1)) }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_loadimm(regs: *mut i64) -> i64 {
    unsafe {
        let dest = load_hole!(HOLE_1);
        let imm = load_hole!(HOLE_IMM) as i64;
        *regs.add(dest) = imm;
        become tj_marker(regs)
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_move(regs: *mut i64) -> i64 {
    unsafe {
        let dest = load_hole!(HOLE_1);
        let src = load_hole!(HOLE_2);
        *regs.add(dest) = *regs.add(src);
        become tj_marker(regs)
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_add(regs: *mut i64) -> i64 {
    unsafe {
        let dest = load_hole!(HOLE_1);
        let a = load_hole!(HOLE_2);
        let b = load_hole!(HOLE_3);
        *regs.add(dest) = *regs.add(a) + *regs.add(b);
        become tj_marker(regs)
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_addimm(regs: *mut i64) -> i64 {
    unsafe {
        let dest = load_hole!(HOLE_1);
        let a = load_hole!(HOLE_2);
        let imm = load_hole!(HOLE_IMM) as i64;
        *regs.add(dest) = *regs.add(a) + imm;
        become tj_marker(regs)
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_jmpzn(regs: *mut i64) -> i64 {
    unsafe {
        let reg = load_hole!(HOLE_1);
        let cond = *regs.add(reg);
        if cond <= 0 {
            become tj_marker(regs)
        } else {
            become tj_marker2(regs)
        }
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_jmp(regs: *mut i64) -> i64 {
    unsafe { become tj_marker(regs) }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "rust-preserve-none" fn tj_ret(regs: *mut i64) -> i64 {
    unsafe {
        let reg = load_hole!(HOLE_1);
        *regs.add(reg)
    }
}
