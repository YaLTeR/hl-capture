#[derive(Debug, Clone, Copy)]
pub struct Function<F> {
    ptr: F,
}

gen_function_impls!(a: A, b: B, c: C, d: D, e: E, f: F);
