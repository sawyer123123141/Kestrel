//! `try` and `let x = r else why { }` (the language round, decisions 9 and 10).

mod common;
use common::*;

#[test]
fn try_is_a_prefix_operator_at_unary_precedence() {
    assert_eq!(body("let x = try a.b(c) + 1"), "{(let x (+ (try (.call a b c)) 1))}");
    assert_eq!(body("let y = try f(x) * try g()"), "{(let y (* (try (call f x)) (try (call g))))}");
    assert_eq!(body("try file.write(p, b)"), "{(try (.call file write p b))}");
    assert_eq!(body("let z = -try f()"), "{(let z (- (try (call f))))}");
    assert_eq!(body("let w = try xs[0].v"), "{(let w (try (. (index xs 0) v)))}");
}

#[test]
fn let_else_names_the_error() {
    assert_eq!(body("let x = r else why {\n print(why)\n return\n}"), "{(let x r else why {(call print why); return})}");
    assert_eq!(body("let x = r else { return }"), "{(let x r else {return})}");
}

#[test]
fn try_is_a_keyword() {
    assert_eq!(errs("main() {\n let try = 3\n}\n")[0], "expected a variable name, found `try`");
}
