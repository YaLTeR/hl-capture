/// A container for a FFI function pointer.
///
/// `Function<F>` ensures the stored pointer is valid at all times.
/// By default it is set to a special function which panics upon being called.
/// This, unfortunately, results in varargs functions not being storable.
#[derive(Debug, Clone, Copy)]
pub struct Function<F> {
    /// The stored function pointer.
    ptr: F,
}

gen_function_impls!(a: A, b: B, c: C, d: D, e: E, f: F);
