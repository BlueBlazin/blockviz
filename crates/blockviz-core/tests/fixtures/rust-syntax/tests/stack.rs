use syntax_tour::Stack;

#[test]
fn pushes() {
    let mut s: Stack<u8, 1> = Stack::new();
    assert!(s.push(1).is_ok());
}
