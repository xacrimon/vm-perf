use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use vm_perf::{
    Bytecode, BytecodeClosures, BytecodeRegister, BytecodeRegisterBecome, ClosureContinuations,
    ClosureStackContinuations, Closures, Expr, RegisterClosures, StackClosures, TapeClosures,
    TapeContinuations, TemplateJit, Vm, Walker,
};

fn create_expr() -> Expr {
    // let mut total = 0;
    // let mut count = args[0];
    // while count > 0 {
    //     total = total + args[1];
    //     count = count - 1;
    // }
    // total
    Expr::Let(
        Box::new(Expr::Litr(0)), // total
        Box::new(Expr::Then(
            Box::new(Expr::Let(
                Box::new(Expr::Arg(0)), // counter
                Box::new(Expr::While(
                    Box::new(Expr::Get(0)),
                    Box::new(Expr::Then(
                        Box::new(Expr::Set(
                            1,
                            Box::new(Expr::Add(Box::new(Expr::Get(1)), Box::new(Expr::Arg(1)))),
                        )),
                        Box::new(Expr::Set(
                            0,
                            Box::new(Expr::Add(Box::new(Expr::Get(0)), Box::new(Expr::Litr(-1)))),
                        )),
                    )),
                )),
            )),
            Box::new(Expr::Get(0)), // total
        )),
    )
}

#[inline(never)]
unsafe fn rust_impl(args: &[i64]) -> i64 {
    // Lots of silly stuff to force the compiler skip basically all attempts at optimisation
    let mut total = black_box(0);
    let mut count = black_box(*args.get_unchecked(0));
    while black_box(count) > 0 {
        total = black_box(total) + black_box(*args.get_unchecked(1));
        count = black_box(count) + black_box(-1);
    }
    black_box(total)
}

#[inline(never)]
unsafe fn rust_impl_opt(args: &[i64]) -> i64 {
    let mut total = 0;
    let mut count = *args.get_unchecked(0);
    while count > 0 {
        total = total + *args.get_unchecked(1);
        count = count + -1;
    }
    total
}

fn create_args() -> &'static [i64] {
    &[10000, 13]
}

fn answer() -> i64 {
    10000 * 13
}

fn bench_compile<V: Vm>(c: &mut Criterion, name: &str) {
    let expr = create_expr();
    c.bench_function(name, |b| {
        b.iter(|| {
            black_box(V::compile(black_box(&expr)));
        });
    });
}

fn bench_execute<V: Vm>(c: &mut Criterion, name: &str) {
    let expr = create_expr();
    let program = V::compile(&expr);
    let args = create_args();
    c.bench_function(name, |b| {
        b.iter(|| {
            let res = unsafe { V::execute(black_box(&program), black_box(args)) };
            assert_eq!(res, answer());
            res
        });
    });
}

fn all_benches(c: &mut Criterion) {
    // AST walker
    bench_compile::<Walker>(c, "walker_compile");
    bench_execute::<Walker>(c, "walker_execute");
    // Bytecode
    bench_compile::<Bytecode>(c, "bytecode_compile");
    bench_execute::<Bytecode>(c, "bytecode_execute");
    // Bytecode register
    bench_compile::<BytecodeRegister>(c, "bytecode_register_compile");
    bench_execute::<BytecodeRegister>(c, "bytecode_register_execute");
    // Bytecode register (tail-call dispatch)
    bench_compile::<BytecodeRegisterBecome>(c, "bytecode_register_become_compile");
    bench_execute::<BytecodeRegisterBecome>(c, "bytecode_register_become_execute");
    // Template JIT (copy-and-patch)
    bench_compile::<TemplateJit>(c, "template_jit_compile");
    bench_execute::<TemplateJit>(c, "template_jit_execute");
    // Closures
    bench_compile::<Closures>(c, "closures_compile");
    bench_execute::<Closures>(c, "closures_execute");
    // Stack closures
    bench_compile::<StackClosures>(c, "stack_closures_compile");
    bench_execute::<StackClosures>(c, "stack_closures_execute");
    // Tape closures
    bench_compile::<TapeClosures>(c, "tape_closures_compile");
    bench_execute::<TapeClosures>(c, "tape_closures_execute");
    // Register closures
    bench_compile::<RegisterClosures>(c, "register_closures_compile");
    bench_execute::<RegisterClosures>(c, "register_closures_execute");
    // Bytecode closures
    bench_compile::<BytecodeClosures>(c, "bytecode_closures_compile");
    bench_execute::<BytecodeClosures>(c, "bytecode_closures_execute");
    // Tape continuations
    bench_compile::<TapeContinuations>(c, "tape_continuations_compile");
    bench_execute::<TapeContinuations>(c, "tape_continuations_execute");
    // Closure continuations
    bench_compile::<ClosureContinuations>(c, "closure_continuations_compile");
    bench_execute::<ClosureContinuations>(c, "closure_continuations_execute");
    // Closure stack continuations
    bench_compile::<ClosureStackContinuations>(c, "closure_stack_continuations_compile");
    bench_execute::<ClosureStackContinuations>(c, "closure_stack_continuations_execute");

    // Pure Rust controls
    let args = create_args();
    c.bench_function("rust_execute", |b| {
        b.iter(|| {
            let res = unsafe { rust_impl(black_box(args)) };
            assert_eq!(res, answer());
            res
        });
    });
    c.bench_function("rust_opt_execute", |b| {
        b.iter(|| {
            let res = unsafe { rust_impl_opt(black_box(args)) };
            assert_eq!(res, answer());
            res
        });
    });
}

criterion_group!(benches, all_benches);
criterion_main!(benches);
