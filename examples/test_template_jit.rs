use vm_perf::{Expr, TemplateJit, Vm};

fn create_expr() -> Expr {
    Expr::Let(
        Box::new(Expr::Litr(0)),
        Box::new(Expr::Then(
            Box::new(Expr::Let(
                Box::new(Expr::Arg(0)),
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
            Box::new(Expr::Get(0)),
        )),
    )
}

fn main() {
    let expr = create_expr();
    let prog = TemplateJit::compile(&expr);
    let args = [10000i64, 13];
    let result = unsafe { TemplateJit::execute(&prog, &args) };
    println!("result = {result}, expected = {}", 10000 * 13);
    assert_eq!(result, 10000 * 13);
    println!("SUCCESS");
}
