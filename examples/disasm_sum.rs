use vm_perf::{BytecodeRegisterBecome, Expr, Vm};

// Mirrors `create_expr` in benches/sum.rs.
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

fn main() {
    let expr = create_expr();
    let program = BytecodeRegisterBecome::compile(&expr);
    println!("{program}");
}
