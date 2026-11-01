//! Hover and go to definition with modules in folders: a dotted `use`, a full name in a type, and
//! an item reached through a module's last name.

use std::path::Path;

use kestrel_driver::{analyze, Analysis};
use kestrel_lsp::resolve::resolve;

const MAIN: &str = "use net.packet

main() {
    let p: net.packet.Packet = packet.make(2)
    print(p.n)
}
";

const PACKET: &str = "pub struct Packet {
    n: int
}

pub make(n: int) -> Packet {
    Packet(n: n)
}
";

fn analysed() -> Analysis {
    let read = |p: &Path| p.ends_with("net/packet.kes").then(|| PACKET.to_string());
    let a = analyze("game/main.kes", MAIN, true, &read);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    a
}

/// The offset of the first `needle` in `MAIN`, plus `into` bytes.
fn at(needle: &str, into: u32) -> u32 {
    MAIN.find(needle).unwrap_or_else(|| panic!("no {needle}")) as u32 + into
}

/// The text a symbol's definition covers, and which source it is in.
fn defined(a: &Analysis, offset: u32) -> (usize, String) {
    let def = resolve(a, offset).unwrap_or_else(|| panic!("nothing at {offset}")).def.expect("a definition");
    let i = a.source_at(def.start);
    let s = &a.sources[i];
    (i, s.src[(def.start - s.base) as usize..(def.end - s.base) as usize].to_string())
}

#[test]
fn a_dotted_use_leads_to_the_folder_file() {
    let a = analysed();
    assert!(a.sources[1].path.ends_with("packet.kes"));
    assert_eq!(a.sources[1].module, "net.packet");
    // On either name of `net.packet`.
    for into in [1, 6] {
        let m = resolve(&a, at("net.packet\n", into)).expect("the module");
        assert_eq!(m.code, "module net.packet");
        assert_eq!(a.source_at(m.def.unwrap().start), 1);
    }
}

#[test]
fn each_name_of_a_full_type_name_leads_to_the_type() {
    let a = analysed();
    for into in [1, 5, 12] {
        let t = resolve(&a, at("net.packet.Packet =", into)).expect("the type");
        assert!(t.code.starts_with("struct net.packet.Packet {"), "{}", t.code);
        assert_eq!(defined(&a, at("net.packet.Packet =", into)), (1, "Packet".to_string()));
    }
}

#[test]
fn a_modules_last_name_leads_to_its_item() {
    let a = analysed();
    // `packet` and `make` of `packet.make(2)` both name the function.
    for into in [1, 8] {
        assert_eq!(defined(&a, at("packet.make", into)), (1, "make".to_string()));
    }
}
