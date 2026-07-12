use std::fmt::Write as _;
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

fn sanitize(s: &str) -> String {
    s.replace('-', "neg")
}

fn main() {
    let expr = create_expr();
    let prog = TemplateJit::compile(&expr);
    let code = prog.code();
    let labels = prog.labels();

    let mut out = String::new();
    writeln!(out, ".section __TEXT,__text,regular,pure_instructions").unwrap();
    writeln!(out, ".globl _sum_program").unwrap();
    writeln!(out, "_sum_program:").unwrap();

    let mut label_idx = 0;
    for (i, byte) in code.iter().enumerate() {
        if label_idx < labels.len() && labels[label_idx].0 == i {
            // Deliberately NOT prefixed with `L`: Mach-O `as` treats `L`-prefixed
            // labels as temporary and strips them before the symbol table, which is
            // exactly what we don't want here — we want objdump to show each one.
            writeln!(out, "op_{}:", sanitize(&labels[label_idx].1)).unwrap();
            label_idx += 1;
        }
        writeln!(out, "    .byte 0x{byte:02x}").unwrap();
    }

    let out_path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/sum_program.s".to_string());
    std::fs::write(&out_path, out).unwrap();
    eprintln!("wrote {out_path} ({} bytes of code)", code.len());
}
