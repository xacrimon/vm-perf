use super::*;
use std::mem;

pub struct BytecodeRegister;

// Linear form produced by `compile_inner`. `JmpZN`/`Jmp` targets are indices into
// this same instruction list, resolved into direct pointers by the "linking" pass
// in `compile` to produce `Instr`.
enum Op {
    LoadImm(usize, i64),
    LoadArg(usize, usize),
    Move(usize, usize),
    Add(usize, usize, usize),
    JmpZN(usize, usize),
    Jmp(usize),
    Ret(usize),
}

// Threaded form actually executed: `JmpZN`/`Jmp` carry a direct pointer to their
// target instruction instead of an index, so a handler never needs to know the
// base of the instruction stream to jump around in it.
pub enum Instr {
    LoadImm(usize, i64),
    LoadArg(usize, usize),
    Move(usize, usize),
    Add(usize, usize, usize),
    JmpZN(usize, *const Instr),
    Jmp(*const Instr),
    Ret(usize),
}

pub struct Program {
    // Self-referential: elements of this boxed slice point back into it. Safe because
    // a `Box<[Instr]>`'s backing allocation never moves for the life of the box, even
    // as the `Box` value itself (and the `Program` it lives in) is moved around.
    ops: Box<[Instr]>,
    num_regs: usize,
}

impl Vm for BytecodeRegister {
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

        // `depth` is the number of `Let`-bound locals currently in scope; each local
        // is permanently assigned register `binding_depth` for the extent of its scope,
        // mirroring how the stack VM's `locals` grows/shrinks with nesting.
        //
        // `next_reg` is a bump watermark for temporaries, always >= depth. Returns the
        // register holding the expression's result, plus whether that register is a
        // fresh temporary the caller is free to overwrite (as opposed to a local's
        // register, which must not be clobbered since it may be read again later).
        fn compile_inner(ctx: &mut Ctx, expr: &Expr, depth: usize, next_reg: usize) -> (usize, bool) {
            match expr {
                Expr::Litr(x) => {
                    ctx.ops.push(Op::LoadImm(next_reg, *x));
                    ctx.touch(next_reg);
                    (next_reg, true)
                }
                Expr::Arg(idx) => {
                    ctx.ops.push(Op::LoadArg(next_reg, *idx));
                    ctx.touch(next_reg);
                    (next_reg, true)
                }
                Expr::Get(local) => (depth - 1 - local, false),
                Expr::Add(x, y) => {
                    let (rx, x_temp) = compile_inner(ctx, x, depth, next_reg);
                    let y_reg = if x_temp { rx + 1 } else { next_reg };
                    let (ry, _) = compile_inner(ctx, y, depth, y_reg);
                    let dest = next_reg;
                    ctx.ops.push(Op::Add(dest, rx, ry));
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
                    let (r_rhs, _) = compile_inner(ctx, rhs, depth, depth);
                    if r_rhs != target {
                        ctx.ops.push(Op::Move(target, r_rhs));
                    }
                    (target, false)
                }
                Expr::While(pred, body) => {
                    let start = ctx.ops.len();
                    let (r_pred, _) = compile_inner(ctx, pred, depth, depth);
                    let branch_fixup = ctx.ops.len();
                    ctx.ops.push(Op::JmpZN(r_pred, 0)); // Will be fixed up
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

        let mut ctx = Ctx {
            ops: Vec::new(),
            max_reg: 0,
        };

        let (result, _) = compile_inner(&mut ctx, expr, 0, 0);
        ctx.ops.push(Op::Ret(result));

        // Link: allocate the final instruction array (with jump targets set to a
        // placeholder), then patch each jump in place with a pointer into that same,
        // now address-stable, allocation.
        let mut ops: Box<[Instr]> = ctx
            .ops
            .iter()
            .map(|op| match *op {
                Op::LoadImm(dest, x) => Instr::LoadImm(dest, x),
                Op::LoadArg(dest, idx) => Instr::LoadArg(dest, idx),
                Op::Move(dest, src) => Instr::Move(dest, src),
                Op::Add(dest, a, b) => Instr::Add(dest, a, b),
                Op::JmpZN(reg, _) => Instr::JmpZN(reg, std::ptr::null()),
                Op::Jmp(_) => Instr::Jmp(std::ptr::null()),
                Op::Ret(reg) => Instr::Ret(reg),
            })
            .collect();

        let base = ops.as_ptr();
        for (i, op) in ctx.ops.iter().enumerate() {
            match *op {
                Op::JmpZN(reg, target) => ops[i] = Instr::JmpZN(reg, unsafe { base.add(target) }),
                Op::Jmp(target) => ops[i] = Instr::Jmp(unsafe { base.add(target) }),
                _ => {}
            }
        }

        Program {
            ops,
            num_regs: ctx.max_reg.max(1),
        }
    }

    unsafe fn execute(prog: &Self::Program<'_>, args: &[i64]) -> i64 {
        let mut regs = vec![0i64; prog.num_regs];
        dispatch(regs.as_mut_ptr(), args.as_ptr(), prog.ops.as_ptr())
    }
}

type OpFn = extern "rust-preserve-none" fn(*mut i64, *const i64, *const Instr) -> i64;

static DISPATCH_TABLE: [OpFn; 7] = [
    op_loadimm,
    op_loadarg,
    op_move,
    op_add,
    op_jmpzn,
    op_jmp,
    op_ret,
];

// SAFETY: for a plain enum with no explicit discriminant values, rustc numbers
// discriminants 0, 1, 2, ... in declaration order, and `Discriminant<T>`'s
// representation for any enum is a bare `u64` holding that value. This gives an
// O(1) index into `DISPATCH_TABLE` matching `Instr`'s variant order above.
#[inline(always)]
fn op_index(instr: &Instr) -> usize {
    unsafe { mem::transmute_copy::<mem::Discriminant<Instr>, u64>(&mem::discriminant(instr)) as usize }
}

#[inline(always)]
extern "rust-preserve-none" fn dispatch(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let instr = unsafe { &*ip };
    let f = unsafe { *DISPATCH_TABLE.get_unchecked(op_index(instr)) };
    become f(regs, args, ip)
}

#[inline(never)]
extern "rust-preserve-none" fn op_loadimm(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let Instr::LoadImm(dest, x) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    unsafe {
        *regs.add(*dest) = *x;
    }
    let next = unsafe { ip.add(1) };
    become dispatch(regs, args, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_loadarg(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let Instr::LoadArg(dest, idx) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    unsafe {
        *regs.add(*dest) = *args.add(*idx);
    }
    let next = unsafe { ip.add(1) };
    become dispatch(regs, args, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_move(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let Instr::Move(dest, src) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    unsafe {
        *regs.add(*dest) = *regs.add(*src);
    }
    let next = unsafe { ip.add(1) };
    become dispatch(regs, args, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_add(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let Instr::Add(dest, a, b) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    unsafe {
        let av = *regs.add(*a);
        let bv = *regs.add(*b);
        *regs.add(*dest) = av + bv;
    }
    let next = unsafe { ip.add(1) };
    become dispatch(regs, args, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_jmpzn(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let Instr::JmpZN(reg, target) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    let cond = unsafe { *regs.add(*reg) };
    let next = if cond <= 0 { *target } else { unsafe { ip.add(1) } };
    become dispatch(regs, args, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_jmp(regs: *mut i64, args: *const i64, ip: *const Instr) -> i64 {
    let Instr::Jmp(target) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    let next = *target;
    become dispatch(regs, args, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_ret(regs: *mut i64, _args: *const i64, ip: *const Instr) -> i64 {
    let Instr::Ret(reg) = (unsafe { &*ip }) else {
        unsafe { std::hint::unreachable_unchecked() }
    };
    unsafe { *regs.add(*reg) }
}
