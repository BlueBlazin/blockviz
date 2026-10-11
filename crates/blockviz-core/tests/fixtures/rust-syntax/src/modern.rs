//! Edition 2021–2024 syntax: let-else, let chains, async closures, GATs,
//! const generics, precise capturing, raw references, inline const.
use std::collections::HashMap;
use std::fmt::{self, Display};
use std::future::Future;

/// A shape with an area.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Circle { radius: f64 },
    Square(f64),
    Point,
    Custom(Box<dyn Fn(f64) -> f64>) = 0,
}

impl Shape {
    pub const UNIT: Shape = Shape::Square(1.0);

    /// The area it covers.
    pub fn area(&self) -> f64 {
        match self {
            Shape::Circle { radius } if *radius > 0.0 => std::f64::consts::PI * radius * radius,
            Shape::Circle { .. } => 0.0,
            Shape::Square(side) => side * side,
            Shape::Point | Shape::Custom(_) => 0.0,
        }
    }
}

impl Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} with area {area:.2}", self, area = self.area())
    }
}

/// A stack with a fixed capacity.
pub struct Stack<T, const N: usize> {
    items: [Option<T>; N],
    len: usize,
}

impl<T: Copy, const N: usize> Stack<T, N> {
    pub fn new() -> Self {
        Self { items: [None; N], len: 0 }
    }

    pub fn push(&mut self, item: T) -> Result<(), T> {
        let Some(slot) = self.items.get_mut(self.len) else {
            return Err(item);
        };
        *slot = Some(item);
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        if let Some(top) = self.len.checked_sub(1)
            && let Some(item) = self.items[top].take()
        {
            self.len = top;
            return Some(item);
        }
        None
    }
}

pub trait Lender {
    type Item<'a> where Self: 'a;
    fn lend(&mut self) -> Option<Self::Item<'_>>;
    fn count_all(&mut self) -> usize where Self: Sized {
        let mut n = 0;
        while let Some(_) = self.lend() {
            n += 1;
        }
        n
    }
}

pub trait Service {
    async fn call(&self, request: String) -> String;
}

pub fn captures<'a>(x: &'a str) -> impl Iterator<Item = char> + use<'a> {
    x.chars()
}

pub fn raw_and_const() -> usize {
    let mut value = 5u32;
    let pointer = &raw mut value;
    let size = const { std::mem::size_of::<u64>() };
    let label = 'outer: {
        if size > 4 {
            break 'outer "big";
        }
        "small"
    };
    let c_string = c"hello";
    let raw = r#"a "quoted" string"#;
    let bytes = b"bytes";
    let r#type = "raw identifier";
    size + label.len() + c_string.count_bytes() + raw.len() + bytes.len() + r#type.len()
}

pub async fn run_all<F>(tasks: Vec<F>) -> Vec<u32>
where
    F: Future<Output = u32>,
{
    let mut out = Vec::with_capacity(tasks.len());
    for task in tasks {
        out.push(task.await);
    }
    let double = async |x: u32| x * 2;
    let mut doubled = vec![];
    for x in &out {
        doubled.push(double(*x).await);
    }
    doubled
}

pub fn counts(words: &[&str]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for word in words.iter().filter(|w| !w.is_empty()) {
        *counts.entry(word.to_lowercase()).or_insert(0) += 1;
    }
    let total: usize = counts.values().sum();
    let lengths: Vec<usize> = words.iter().map(|w| w.len()).collect();
    let squares = (0..total).map(|i| i * i).collect::<Vec<_>>();
    debug_assert!(lengths.len() + squares.len() >= total);
    counts
}

pub unsafe extern "C" fn exported(x: i32) -> i32 {
    unsafe { x.unchecked_add(1) }
}

unsafe extern "C" {
    pub safe fn abs(input: i32) -> i32;
}

#[unsafe(no_mangle)]
pub extern "C" fn no_mangle() {}

pub fn never() -> ! {
    loop {
        std::thread::yield_now();
    }
}

pub struct Wrapper(pub Vec<u8>);

pub union Bits {
    int: u32,
    float: f32,
}

pub fn ranges(x: u8) -> &'static str {
    match x {
        0 => "zero",
        1..=9 => "digit",
        10.. => "big",
    }
}

pub fn closures() -> impl Fn(i32) -> i32 {
    let offset = 10;
    move |x| {
        let y = x + offset;
        y * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_pushes_and_pops() {
        let mut s: Stack<u8, 2> = Stack::new();
        assert!(s.push(1).is_ok());
        assert_eq!(s.pop(), Some(1));
    }
}
