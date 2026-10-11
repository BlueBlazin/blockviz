// A file with a syntax error in one function: the rest still shows.
pub fn fine() -> u32 {
    1
}

pub fn broken() -> u32 {
    let x = ;
    x
}

pub struct StillHere {
    pub value: u32,
}
