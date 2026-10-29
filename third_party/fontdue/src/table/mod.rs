mod gsub;
mod kern;
pub mod parse;

#[allow(unused_imports)] // pbanki patch: glyphs load lazily, GSUB preloading is unused.
pub use self::gsub::load_gsub;
pub use self::kern::*;
