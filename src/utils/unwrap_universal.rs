// on web, unwrap() is not recommended
// instead, wasm_bindgen's unwrap_throw() is recommended, which throws a JavaScript error
// however, always writing two versions of the same code, one with unwrap() and one with unwrap_throw(), is tedious
// so, just always use unwrap_universal() defined here
pub trait UnwrapUniversal<T> {
    fn unwrap_universal(self) -> T;
}

#[cfg(target_arch = "wasm32")]
impl<T> UnwrapUniversal<T> for Option<T> {
    fn unwrap_universal(self) -> T {
        // unwrap_throw() is not part of std, but is defined by wasm_bindgen::UnwrapThrowExt trait
        // UnwrapThrowExt adds unwrap_throw() to Option<T> and to any Result<T, E> where E implements Debug trait
        use wasm_bindgen::UnwrapThrowExt;
        self.unwrap_throw()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<T> UnwrapUniversal<T> for Option<T> {
    fn unwrap_universal(self) -> T {
        self.unwrap()
    }
}

#[cfg(target_arch = "wasm32")]
impl<T, E: core::fmt::Debug> UnwrapUniversal<T> for Result<T, E> {
    fn unwrap_universal(self) -> T {
        use wasm_bindgen::UnwrapThrowExt;
        self.unwrap_throw()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<T, E: core::fmt::Debug> UnwrapUniversal<T> for Result<T, E> {
    fn unwrap_universal(self) -> T {
        self.unwrap()
    }
}
