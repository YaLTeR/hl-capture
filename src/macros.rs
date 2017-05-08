macro_rules! cstr {
    ($s:expr) => ($s as *const _ as *const ::libc::c_char)
}

macro_rules! real {
    ($f:ident) => ({
        let func = ::hooks::hw::POINTERS.read().unwrap().$f.get();
        func
    })
}

macro_rules! find {
    ($pointers:ident, $handle:ident, $f:ident, $symbol:tt) => ({
        let ptr = $handle.sym($symbol)
            .chain_err(|| concat!("couldn't find ", stringify!($f), "()"))?;
        $pointers.$f.set_from_raw(ptr);
    })
}

macro_rules! command {
    ($name:ident, $command:tt, $callback:expr) => (
        #[allow(non_camel_case_types)]
        struct $name;

        impl $name {
            extern "C" fn callback() {
                const F: &Fn(::engine::Engine) = &$callback;

                // We know this is the main game thread.
                let engine = unsafe { ::engine::Engine::new() };

                F(engine);
            }

            // This will be called at .so init time. Add this command to the global list.
            #[allow(dead_code)]
            extern "C" fn initialize() {
                #[link_section=".init_array"]
                static INITIALIZE: extern "C" fn() = $name::initialize;

                ::command::COMMANDS.write().unwrap().push(Box::new($name));
            }
        }

        impl ::command::Command for $name {
            fn name(&self) -> &'static [u8] {
                $command
            }

            fn callback(&self) -> extern "C" fn() {
                Self::callback
            }
        }
    )
}

macro_rules! gen_function_impls {
    (@make_impl ($($extern_type:tt)*) ($($arg_name:ident : $arg_type:ident),*)) => (
        impl<R $(, $arg_type)*> Default for Function<$($extern_type)* fn($($arg_type),*) -> R> {
            #[inline(always)]
            fn default() -> Self {
                Function {
                    ptr: Self::default_func as $($extern_type)* fn($($arg_type),*) -> R,
                }
            }
        }

        #[allow(dead_code)]
        impl<R $(, $arg_type)*> Function<$($extern_type)* fn($($arg_type),*) -> R> {
            #[inline(always)]
            pub fn is_default(&self) -> bool {
                self.ptr as *const usize == Self::default_func as *const usize
            }

            #[inline(always)]
            pub unsafe fn call(&self $(, $arg_name : $arg_type)*) -> R {
                (self.ptr)($($arg_name),*)
            }

            #[inline(always)]
            pub fn get(&self) -> $($extern_type)* fn($($arg_type),*) -> R {
                self.ptr
            }

            #[inline(always)]
            pub fn set(&mut self, f: $($extern_type)* fn($($arg_type),*) -> R) {
                self.ptr = f;
            }

            #[inline(always)]
            pub unsafe fn set_from_raw(&mut self, f: *const ::libc::c_void) {
                self.set(*(&f as *const _ as *const _));
            }

            // This should never be called.
            $($extern_type)* fn default_func($(_: $arg_type),*) -> R {
                unreachable!();
            }
        }
    );

    (@gen_impls $($arg_name:ident : $arg_type:ident),*) => (
        gen_function_impls!(@make_impl (unsafe                  ) ($($arg_name : $arg_type),*));
        gen_function_impls!(@make_impl (unsafe extern "C"       ) ($($arg_name : $arg_type),*));
        gen_function_impls!(@make_impl (unsafe extern "system"  ) ($($arg_name : $arg_type),*));
        gen_function_impls!(@make_impl (unsafe extern "fastcall") ($($arg_name : $arg_type),*));
    );

    () => (
        gen_function_impls!(@gen_impls);
    );

    ($first_arg_name:ident : $first_arg_type:ident $(, $arg_name:ident : $arg_type:ident)*) => (
        gen_function_impls!(@gen_impls $first_arg_name : $first_arg_type $(, $arg_name : $arg_type)*);
        gen_function_impls!($($arg_name : $arg_type),*);
    );
}
