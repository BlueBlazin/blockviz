#!/usr/bin/env rust-script
use syntax_tour::modern::Shape;

fn main() {
    let shape = Shape::Square(2.0);
    println!("{}", shape.area());
}
