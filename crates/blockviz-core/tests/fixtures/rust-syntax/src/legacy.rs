// Edition 2015 style: extern crate, try!, bare trait objects, `...` ranges.
extern crate std as stdlib;

use std::error::Error;
use std::fs::File;
use std::io::Read;

pub fn read_all(path: &str) -> Result<String, Box<Error>> {
    let mut file = try!(File::open(path));
    let mut text = String::new();
    try!(file.read_to_string(&mut text));
    Ok(text)
}

pub fn classify(c: u8) -> &'static str {
    match c {
        b'a'...b'z' => "lower",
        b'A'...b'Z' => "upper",
        _ => "other",
    }
}

pub fn absolute() -> usize {
    ::std::mem::size_of::<u32>()
}
