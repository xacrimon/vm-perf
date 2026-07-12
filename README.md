# VM Performance Comparison

This repository exists as an accessible benchmark comparison between various strategies for implementing interpreters.

The benchmarks are not particularly scientific. Take them with a pinch of salt.

## Benchmarks

Benchmarks were performed on a 16 core AMD Ryzen 7 3700X.

```
test bytecode_closures_compile           ... bench:         263.61 ns/iter (+/- 20.72)
test bytecode_closures_execute           ... bench:     136,063.45 ns/iter (+/- 10,884.46)

test bytecode_compile                    ... bench:         128.76 ns/iter (+/- 13.41)
test bytecode_execute                    ... bench:      95,541.25 ns/iter (+/- 8,267.20)

test bytecode_register_compile           ... bench:          94.96 ns/iter (+/- 8.63)
test bytecode_register_execute           ... bench:      40,637.19 ns/iter (+/- 4,585.22)

test bytecode_register_become_compile    ... bench:         126.42 ns/iter (+/- 6.66)
test bytecode_register_become_execute    ... bench:      25,092.47 ns/iter (+/- 2,660.43)

test closure_continuations_compile       ... bench:          93.92 ns/iter (+/- 6.42)
test closure_continuations_execute       ... bench:      24,462.99 ns/iter (+/- 383.64)

test closure_stack_continuations_compile ... bench:         104.73 ns/iter (+/- 8.26)
test closure_stack_continuations_execute ... bench:      28,798.22 ns/iter (+/- 2,088.12)

test closures_compile                    ... bench:          82.87 ns/iter (+/- 9.15)
test closures_execute                    ... bench:      38,680.54 ns/iter (+/- 1,061.34)

test register_closures_compile           ... bench:         152.99 ns/iter (+/- 10.11)
test register_closures_execute           ... bench:      39,213.61 ns/iter (+/- 823.45)

test rust_execute                        ... bench:      26,944.06 ns/iter (+/- 2,105.88)
test rust_opt_execute                    ... bench:           0.70 ns/iter (+/- 0.04)

test stack_closures_compile              ... bench:         272.29 ns/iter (+/- 26.45)
test stack_closures_execute              ... bench:     137,509.03 ns/iter (+/- 5,658.82)

test tape_closures_compile               ... bench:         109.04 ns/iter (+/- 7.66)
test tape_closures_execute               ... bench:      92,271.36 ns/iter (+/- 2,685.96)

test tape_continuations_compile          ... bench:         109.42 ns/iter (+/- 10.74)
test tape_continuations_execute          ... bench:      27,725.32 ns/iter (+/- 980.05)

test walker_compile                      ... bench:           0.22 ns/iter (+/- 0.00)
test walker_execute                      ... bench:     107,587.50 ns/iter (+/- 6,311.80)
```

`rust_execute` and `rust_opt_execute` are 'standard candles', implemented in native Rust code. The former has very few
optimisations applied, whereas the latter is permitted to take advantage of the full optimising power of LLVM.

The fastest technique appears to be [`closure_continuations`](#closure_continuations). It manages to achieve very
respectable performance, coming within spitting difference of (deoptimised) native code.

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
