use super::*;
use std::fmt;

pub struct BytecodeRegister;

enum Op {
    LoadImm(usize, i64),
    Move(usize, usize),
    Add(usize, usize, usize),
    AddImm(usize, usize, i64),
    JmpZN(usize, usize),
    Jmp(usize),
    Ret(usize),
}

const OP_LOADIMM: u8 = 0;
const OP_MOVE: u8 = 1;
const OP_ADD: u8 = 2;
const OP_ADDIMM: u8 = 3;
const OP_JMPZN: u8 = 4;
const OP_JMP: u8 = 5;
const OP_RET: u8 = 6;

const LOADIMM_SIZE: usize = 1 + 1 + 8;
const MOVE_SIZE: usize = 1 + 1 + 1;
const ADD_SIZE: usize = 1 + 1 + 1 + 1;
const ADDIMM_SIZE: usize = 1 + 1 + 1 + 8;
const JMPZN_SIZE: usize = 1 + 1 + 4;
const JMP_SIZE: usize = 1 + 4;
const RET_SIZE: usize = 1 + 1;

pub struct Program {
    code: Box<[u8]>,
    num_regs: usize,
    n_args: usize,
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "; num_regs = {}, n_args = {}", self.num_regs, self.n_args)?;
        let code = &self.code;
        let mut i = 0usize;
        while i < code.len() {
            write!(f, "{i:4}: ")?;
            match code[i] {
                OP_LOADIMM => {
                    let dest = code[i + 1];
                    let imm = i64::from_ne_bytes(code[i + 2..i + 10].try_into().unwrap());
                    writeln!(f, "loadimm  r{dest}, {imm}")?;
                    i += LOADIMM_SIZE;
                }
                OP_MOVE => {
                    let dest = code[i + 1];
                    let src = code[i + 2];
                    writeln!(f, "move     r{dest}, r{src}")?;
                    i += MOVE_SIZE;
                }
                OP_ADD => {
                    let dest = code[i + 1];
                    let a = code[i + 2];
                    let b = code[i + 3];
                    writeln!(f, "add      r{dest}, r{a}, r{b}")?;
                    i += ADD_SIZE;
                }
                OP_ADDIMM => {
                    let dest = code[i + 1];
                    let a = code[i + 2];
                    let imm = i64::from_ne_bytes(code[i + 3..i + 11].try_into().unwrap());
                    writeln!(f, "addimm   r{dest}, r{a}, {imm}")?;
                    i += ADDIMM_SIZE;
                }
                OP_JMPZN => {
                    let reg = code[i + 1];
                    let offset = i32::from_ne_bytes(code[i + 2..i + 6].try_into().unwrap());
                    let target = i as isize + offset as isize;
                    writeln!(f, "jmpzn    r{reg}, {target:<4} ; offset {offset:+}")?;
                    i += JMPZN_SIZE;
                }
                OP_JMP => {
                    let offset = i32::from_ne_bytes(code[i + 1..i + 5].try_into().unwrap());
                    let target = i as isize + offset as isize;
                    writeln!(f, "jmp      {target:<4} ; offset {offset:+}")?;
                    i += JMP_SIZE;
                }
                OP_RET => {
                    let reg = code[i + 1];
                    writeln!(f, "ret      r{reg}")?;
                    i += RET_SIZE;
                }
                op => unreachable!("bad opcode {op}"),
            }
        }
        Ok(())
    }
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

        #[inline(always)]
        fn reg(r: usize) -> u8 {
            debug_assert!(r <= u8::MAX as usize, "register index {r} exceeds u8 range");
            r as u8
        }

        fn op_size(op: &Op) -> usize {
            match op {
                Op::LoadImm(..) => LOADIMM_SIZE,
                Op::Move(..) => MOVE_SIZE,
                Op::Add(..) => ADD_SIZE,
                Op::AddImm(..) => ADDIMM_SIZE,
                Op::JmpZN(..) => JMPZN_SIZE,
                Op::Jmp(..) => JMP_SIZE,
                Op::Ret(..) => RET_SIZE,
            }
        }

        let mut byte_offset = Vec::with_capacity(ctx.ops.len());
        let mut cursor = 0usize;
        for op in &ctx.ops {
            byte_offset.push(cursor);
            cursor += op_size(op);
        }

        let mut code = Vec::with_capacity(cursor);
        for (i, op) in ctx.ops.iter().enumerate() {
            match *op {
                Op::LoadImm(dest, x) => {
                    code.push(OP_LOADIMM);
                    code.push(reg(dest));
                    code.extend_from_slice(&x.to_ne_bytes());
                }
                Op::Move(dest, src) => {
                    code.push(OP_MOVE);
                    code.push(reg(dest));
                    code.push(reg(src));
                }
                Op::Add(dest, a, b) => {
                    code.push(OP_ADD);
                    code.push(reg(dest));
                    code.push(reg(a));
                    code.push(reg(b));
                }
                Op::AddImm(dest, a, imm) => {
                    code.push(OP_ADDIMM);
                    code.push(reg(dest));
                    code.push(reg(a));
                    code.extend_from_slice(&imm.to_ne_bytes());
                }
                Op::JmpZN(r, target) => {
                    code.push(OP_JMPZN);
                    code.push(reg(r));
                    let offset = byte_offset[target] as isize - byte_offset[i] as isize;
                    debug_assert!(i32::try_from(offset).is_ok(), "jump offset {offset} exceeds i32 range");
                    code.extend_from_slice(&(offset as i32).to_ne_bytes());
                }
                Op::Jmp(target) => {
                    code.push(OP_JMP);
                    let offset = byte_offset[target] as isize - byte_offset[i] as isize;
                    debug_assert!(i32::try_from(offset).is_ok(), "jump offset {offset} exceeds i32 range");
                    code.extend_from_slice(&(offset as i32).to_ne_bytes());
                }
                Op::Ret(r) => {
                    code.push(OP_RET);
                    code.push(reg(r));
                }
            }
        }

        Program {
            code: code.into_boxed_slice(),
            num_regs: ctx.max_reg.max(n_args).max(1),
            n_args,
        }
    }

    unsafe fn execute(prog: &Self::Program<'_>, args: &[i64]) -> i64 {
        let mut regs = vec![0i64; prog.num_regs];
        debug_assert!(args.len() >= prog.n_args, "not enough arguments");
        regs[..prog.n_args].copy_from_slice(&args[..prog.n_args]);
        dispatch(regs.as_mut_ptr(), prog.code.as_ptr())
    }
}

type OpFn = extern "rust-preserve-none" fn(*mut i64, *const u8) -> i64;

static DISPATCH_TABLE: [OpFn; 7] = [
    op_loadimm,
    op_move,
    op_add,
    op_addimm,
    op_jmpzn,
    op_jmp,
    op_ret,
];

#[inline(always)]
extern "rust-preserve-none" fn dispatch(regs: *mut i64, ip: *const u8) -> i64 {
    let opcode = unsafe { *ip };
    let f = unsafe { *DISPATCH_TABLE.get_unchecked(opcode as usize) };
    become f(regs, ip)
}

#[inline(never)]
extern "rust-preserve-none" fn op_loadimm(regs: *mut i64, ip: *const u8) -> i64 {
    let dest = unsafe { *ip.add(1) };
    let imm = unsafe { ip.add(2).cast::<i64>().read_unaligned() };
    unsafe {
        *regs.add(dest as usize) = imm;
    }
    let next = unsafe { ip.add(LOADIMM_SIZE) };
    become dispatch(regs, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_move(regs: *mut i64, ip: *const u8) -> i64 {
    let dest = unsafe { *ip.add(1) };
    let src = unsafe { *ip.add(2) };
    unsafe {
        *regs.add(dest as usize) = *regs.add(src as usize);
    }
    let next = unsafe { ip.add(MOVE_SIZE) };
    become dispatch(regs, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_add(regs: *mut i64, ip: *const u8) -> i64 {
    let dest = unsafe { *ip.add(1) };
    let a = unsafe { *ip.add(2) };
    let b = unsafe { *ip.add(3) };
    unsafe {
        let av = *regs.add(a as usize);
        let bv = *regs.add(b as usize);
        *regs.add(dest as usize) = av + bv;
    }
    let next = unsafe { ip.add(ADD_SIZE) };
    become dispatch(regs, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_addimm(regs: *mut i64, ip: *const u8) -> i64 {
    let dest = unsafe { *ip.add(1) };
    let a = unsafe { *ip.add(2) };
    let imm = unsafe { ip.add(3).cast::<i64>().read_unaligned() };
    unsafe {
        let av = *regs.add(a as usize);
        *regs.add(dest as usize) = av + imm;
    }
    let next = unsafe { ip.add(ADDIMM_SIZE) };
    become dispatch(regs, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_jmpzn(regs: *mut i64, ip: *const u8) -> i64 {
    let reg = unsafe { *ip.add(1) };
    let offset = unsafe { ip.add(2).cast::<i32>().read_unaligned() };
    let cond = unsafe { *regs.add(reg as usize) };
    let next = if cond <= 0 {
        unsafe { ip.offset(offset as isize) }
    } else {
        unsafe { ip.add(JMPZN_SIZE) }
    };
    become dispatch(regs, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_jmp(regs: *mut i64, ip: *const u8) -> i64 {
    let offset = unsafe { ip.add(1).cast::<i32>().read_unaligned() };
    let next = unsafe { ip.offset(offset as isize) };
    become dispatch(regs, next)
}

#[inline(never)]
extern "rust-preserve-none" fn op_ret(regs: *mut i64, ip: *const u8) -> i64 {
    let reg = unsafe { *ip.add(1) };
    unsafe { *regs.add(reg as usize) }
}
