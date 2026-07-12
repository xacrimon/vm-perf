# VM Performance Comparison

This repository exists as an accessible benchmark comparison between various strategies for implementing interpreters.

The benchmarks are not particularly scientific. Take them with a pinch of salt.

## Benchmarks

Benchmarks were performed on an Apple M4 Pro (14 cores: 10 performance + 4 efficiency).

```
test bytecode_closures_compile           ... bench:          271.16 ns/iter (+/- 2.73)
test bytecode_closures_execute           ... bench:      148,760.00 ns/iter (+/- 3,790.00)

test bytecode_compile                    ... bench:          144.96 ns/iter (+/- 1.39)
test bytecode_execute                    ... bench:      102,580.00 ns/iter (+/- 1,615.00)

test bytecode_register_compile           ... bench:          108.53 ns/iter (+/- 1.69)
test bytecode_register_execute           ... bench:       44,249.00 ns/iter (+/- 722.00)

test bytecode_register_become_compile    ... bench:          119.04 ns/iter (+/- 2.01)
test bytecode_register_become_execute    ... bench:       19,838.00 ns/iter (+/- 52.00)

test closure_continuations_compile       ... bench:          109.76 ns/iter (+/- 1.07)
test closure_continuations_execute       ... bench:       27,932.00 ns/iter (+/- 215.00)

test closure_stack_continuations_compile ... bench:          113.99 ns/iter (+/- 1.44)
test closure_stack_continuations_execute ... bench:       33,417.00 ns/iter (+/- 298.50)

test closures_compile                    ... bench:           89.25 ns/iter (+/- 1.08)
test closures_execute                    ... bench:       40,690.00 ns/iter (+/- 414.50)

test register_closures_compile           ... bench:          153.16 ns/iter (+/- 1.45)
test register_closures_execute           ... bench:       43,324.00 ns/iter (+/- 451.50)

test rust_execute                        ... bench:       31,048.00 ns/iter (+/- 152.00)
test rust_opt_execute                    ... bench:            0.74 ns/iter (+/- 0.00)

test stack_closures_compile              ... bench:          281.64 ns/iter (+/- 3.26)
test stack_closures_execute              ... bench:      143,310.00 ns/iter (+/- 1,360.00)

test tape_closures_compile               ... bench:          114.42 ns/iter (+/- 1.20)
test tape_closures_execute               ... bench:       95,800.00 ns/iter (+/- 1,360.50)

test tape_continuations_compile          ... bench:          115.86 ns/iter (+/- 1.17)
test tape_continuations_execute          ... bench:       32,150.00 ns/iter (+/- 157.00)

test template_jit_compile                ... bench:        3,171.70 ns/iter (+/- 99.50)
test template_jit_execute                ... bench:        9,998.80 ns/iter (+/- 1,361.45)

test walker_compile                      ... bench:            0.46 ns/iter (+/- 0.00)
test walker_execute                      ... bench:      114,870.00 ns/iter (+/- 2,615.00)
```

`rust_execute` and `rust_opt_execute` are 'standard candles', implemented in native Rust code. The former has very few
optimisations applied, whereas the latter is permitted to take advantage of the full optimising power of LLVM.

The fastest interpretive technique is `bytecode_register_become`, a register-based bytecode VM dispatched via explicit
tail calls (`become`) instead of a loop-and-match, coming in well ahead of even
[`closure_continuations`](#closure_continuations). Faster still is `template_jit`, which isn't interpreting bytecode at
all — it's a copy-and-patch JIT compiler that assembles a native, executable function per program.

## Setup

Each technique has two stages:

- Compilation: The technique is given an expression AST and is permitted to generate whatever program it needs from it

- Execution: The technique is given the program and told to run the program to completion

For the sake of a fair comparison, I've tried to avoid any techniques taking advantage of the structure of the AST to
improve performance.

The AST provided to the techniques is conceptually simple. The only data types are integers, the only arithmetic
instruction is addition, and the only control flow is `while`. Locals exist and can be created and mutated. Programs
also get provided a series of arguments at execution time to parameterise their execution.

## Techniques

### `walker`

A simple AST walker. Compilation is an identity function. AST evaluation is done by recursively matching on AST nodes.

### `bytecode`

A naive stack 'bytecode' interpreter. Compilation takes the AST and translates it into a list of instructions. Execution
operates upon the stack, pushing and popping values.

### `bytecode_register`

Like `bytecode`, but instructions operate on a fixed set of registers instead of a stack. The compiler assigns each
`Let`-bound local a register based on its nesting depth and bump-allocates further registers for intermediate values, so
a value already sitting in a register is read directly as an operand rather than being pushed and popped through a
stack. Execution is still the same loop-and-`match` dispatch as `bytecode`.

### `bytecode_register_become`

Uses the same register-allocated instruction set as `bytecode_register`, but dispatches differently: each instruction's
handler function ends in an explicit tail call (`become`, an unstable Rust feature) directly to the next instruction's
handler, rather than looping over a `match`. This removes the loop's own overhead from the hot path, leaving only an
indirect jump between handlers. Instructions are encoded as a variable-width byte stream, and function arguments are
pre-loaded into registers up front instead of being read through a separate indirection on every use.

### `closures`

Uses simple indirect threading, 'compiling' the entire program into a deeply nested closure. Execution simply evaluates
the closure.

### `closure_continuations`

Shares much of the simplicity of `closures`, but passes the next instruction to be performed - if any - as a continuation,
allowing for tail-call optimisation (TCO) to occur in a substantial number of cases.

### `closure_stack_continuations`

Just like `closure_continuations`, except it uses a stack to pass values around. This can improve the ability to perform
tail-call optimisations (TCO), at the cost of needing to touch memory when manipulating values. It's possible that some
combination of both approaches might hit an even nicer sweet spot.

### `bytecode_closures`

A mix between `bytecode` and `closures`. The AST is compiled down to a series of instruction-like closures, which are
then executed in a loop and indexed via an instruction pointer.

### `stack_closures`

Like `closures`, except intermediate values are maintained on a `Vec` stack rather than the hardware stack of the
closures.

### `tape_closures`

Like `closures`, except each closure is permitted no environment at compilation time, and instead fetches it from a tape
of static data at execution time.

### `register_closures`

Like `closures`, except the 2 highest most recently created locals are passed through the closures as arguments, rather
than being maintained on the locals stack.

### `tape_continuations`

Similar to `tape_closures`, except the next function to be executed is called from within the previous, allowing the
compiler to perform TCO (Tail Call Optimisation) on the function. This significantly reduces the stack-bashing that
needs to occur to set up each function, resulting in a very significant performance boost: at the cost of complexity.

### `template_jit`

Not an interpreter at all, but a copy-and-patch JIT compiler. A handful of tiny Rust functions ("templates"), one per
instruction kind in `bytecode_register`'s instruction set, are compiled normally by `rustc`. At "compile" time,
`template_jit` reads its own running executable back off disk, copies each template's real machine code into a freshly
allocated executable page, and patches in that instruction's concrete operands and successor address directly into the
copied bytes - assembling one genuinely native function per program. Execution is then just a function call into it, no
dispatch loop involved.
