use super::*;

pub struct BytecodeRegister;

#[derive(Debug)]
pub enum Op {
    LoadImm(usize, i64),
    LoadArg(usize, usize),
    Move(usize, usize),
    Add(usize, usize, usize),
    JmpZN(usize, usize),
    Jmp(usize),
    Ret(usize),
}

pub struct Program {
    ops: Vec<Op>,
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

        let mut ctx = Ctx {
            ops: Vec::new(),
            max_reg: 0,
        };

        let (result, _) = compile_inner(&mut ctx, expr, 0, 0);
        ctx.ops.push(Op::Ret(result));

        Program {
            ops: ctx.ops,
            num_regs: ctx.max_reg.max(1),
        }
    }

    unsafe fn execute(prog: &Self::Program<'_>, args: &[i64]) -> i64 {
        let mut regs = vec![0i64; prog.num_regs];
        let mut ip = 0;
        loop {
            let op = prog.ops.get_unchecked(ip);
            ip += 1;
            match op {
                Op::LoadImm(dest, x) => *regs.get_unchecked_mut(*dest) = *x,
                Op::LoadArg(dest, idx) => *regs.get_unchecked_mut(*dest) = *args.get_unchecked(*idx),
                Op::Move(dest, src) => *regs.get_unchecked_mut(*dest) = *regs.get_unchecked(*src),
                Op::Add(dest, a, b) => {
                    let a = *regs.get_unchecked(*a);
                    let b = *regs.get_unchecked(*b);
                    *regs.get_unchecked_mut(*dest) = a + b;
                }
                Op::JmpZN(reg, target) => {
                    if *regs.get_unchecked(*reg) <= 0 {
                        ip = *target;
                    }
                }
                Op::Jmp(target) => ip = *target,
                Op::Ret(reg) => break *regs.get_unchecked(*reg),
            }
        }
    }
}
