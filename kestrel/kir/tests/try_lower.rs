//! `try` lowers to what the hand-written `match` with a `return` lowers to (the language round,
//! decision 9): a tag test on the `Result`, read in place when it is a place and from a temporary
//! otherwise, a `return` of the error on `Err`, and the `Ok` value read where it is.

use kestrel_kir::*;

fn lowered(src: &str) -> Module {
    let (compiled, text) = kestrel_driver::compile_unoptimized("try.kes", src, kestrel_driver::DiagStyle::Short, true);
    compiled.unwrap_or_else(|| panic!("doesn't compile:\n{text}")).module
}

/// A function's body as text, with every line number taken out (the two functions compared are
/// on different lines).
fn body(m: &Module, name: &str) -> String {
    let f = m.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function `{name}`"));
    let text = format!("{:?}", f.body);
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("line: ") {
        out.push_str(&rest[..at + 6]);
        rest = rest[at + 6..].trim_start_matches(|c: char| c.is_ascii_digit());
        out.push('0');
    }
    out.push_str(rest);
    out
}

const SRC: &str = "pure half(n: int) -> Result[float, int] {
    if n % 2 != 0 {
        return Result.Err(error: n)
    }
    Result.Ok(value: float(n) / 2.0)
}

pure via_try(r: Result[float, int]) -> Result[float, int] {
    let v = try r
    v * 2.0
}

pure via_match(r: Result[float, int]) -> Result[float, int] {
    let v = match r {
        Ok o { o.value }
        Err e { return Result.Err(error: e.error) }
    }
    v * 2.0
}

pure call_try(n: int) -> Result[float, int] {
    let v = try half(n)
    v + 1.0
}

pure call_match(n: int) -> Result[float, int] {
    let v = match half(n) {
        Ok o { o.value }
        Err e { return Result.Err(error: e.error) }
    }
    v + 1.0
}

pure stmt_try(r: Result[float, int]) -> Result[float, int] {
    try r
    1.0
}

pure stmt_match(r: Result[float, int]) -> Result[float, int] {
    match r {
        Ok { }
        Err e { return Result.Err(error: e.error) }
    }
    1.0
}

main() {
    print(via_try(1.0), via_match(1.0), call_try(2), call_match(3), stmt_try(1.0), stmt_match(1.0))
}
";

#[test]
fn try_on_a_place_is_the_written_match() {
    let m = lowered(SRC);
    assert_eq!(body(&m, "via_try"), body(&m, "via_match"));
}

#[test]
fn try_on_a_call_is_the_written_match() {
    let m = lowered(SRC);
    assert_eq!(body(&m, "call_try"), body(&m, "call_match"));
}

#[test]
fn try_as_a_statement_is_the_written_match() {
    let m = lowered(SRC);
    assert_eq!(body(&m, "stmt_try"), body(&m, "stmt_match"));
}
