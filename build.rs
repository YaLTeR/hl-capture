extern crate syn;
extern crate walkdir;

use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use syn::visit::Visitor;

fn main() {
    // Parse all .rs files to collect everything which implements Command.
    // This code won't work properly with lib.rs or mod.rs.
    let mut data = Data::new();

    for entry in walkdir::WalkDir::new("src")
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_type().is_dir()) {
        let mut path = String::new();

        for name in entry.path().with_extension("").iter().skip(1) {
            path.push_str(&format!("::{}", name.to_str().unwrap()));
        }

        let entry_data = get_data(entry.path());

        data.commands.extend(entry_data.commands.into_iter()
                                 .map(|c| format!("{}::{}", &path, c)));
        data.cvars.extend(entry_data.cvars.into_iter()
                              .map(|c| format!("{}::{}", &path, c)));
    }

    let command_array = make_command_array(data.commands);
    let cvar_array = make_cvar_array(data.cvars);

    let out_dir = env::var("OUT_DIR").unwrap();

    let dest_path = Path::new(&out_dir).join("command_array.rs");
    let mut f = File::create(&dest_path).unwrap();
    write!(f, "{}", command_array).unwrap();

    let dest_path = Path::new(&out_dir).join("cvar_array.rs");
    let mut f = File::create(&dest_path).unwrap();
    write!(f, "{}", cvar_array).unwrap();

    // Output OUT_DIR for rustdoc in Travis
    let mut f = File::create("out_dir").unwrap();
    write!(f, "{}", out_dir).unwrap();
}

struct Data {
    commands: Vec<String>,
    cvars: Vec<String>,
}

impl Data {
    fn new() -> Self {
        Self {
            commands: Vec::new(),
            cvars: Vec::new(),
        }
    }
}

fn get_data(path: &Path) -> Data {
    let mut source = String::new();
    File::open(path)
        .unwrap()
        .read_to_string(&mut source)
        .unwrap();

    if let Ok(_crate) = syn::parse_crate(&source) {
        let mut visitor = MyVisitor::new();
        visitor.visit_crate(&_crate);

        Data { commands: visitor.commands, cvars: visitor.cvars }
    } else {
        Data::new()
    }
}

fn make_command_array(commands: Vec<String>) -> String {
    let mut buf = format!("pub const COMMANDS: [&Command; {}] = [", commands.len());

    let mut iter = commands.into_iter();

    if let Some(first) = iter.next() {
        buf.push_str(&format!("&{}", first));
    }

    for command in iter {
        buf.push_str(&format!(", &{}", command));
    }

    buf.push_str("];");

    buf
}

fn make_cvar_array(cvars: Vec<String>) -> String {
    let mut buf = format!("pub static CVARS: [&::std::thread::LocalKey<CVar>; {}] = [",
                          cvars.len());

    let mut iter = cvars.into_iter();

    if let Some(first) = iter.next() {
        buf.push_str(&format!("&{}", first));
    }

    for cvar in iter {
        buf.push_str(&format!(", &{}", cvar));
    }

    buf.push_str("];");

    buf
}

struct MyVisitor {
    commands: Vec<String>,
    cvars: Vec<String>,
}

impl MyVisitor {
    fn new() -> Self {
        Self { commands: Vec::new(), cvars: Vec::new() }
    }
}

impl Visitor for MyVisitor {
    fn visit_mac(&mut self, mac: &syn::Mac) {
        if mac.path == "command".into() {
            if let Some(&syn::TokenTree::Delimited(ref delimited)) = mac.tts.iter().next() {
                if let Some(&syn::TokenTree::Token(ref token)) = delimited.tts.iter().next() {
                    if let &syn::Token::Ident(ref ident) = token {
                        self.commands.push(ident.as_ref().to_owned());
                    }
                }
            }
        }

        if mac.path == "cvar".into() {
            if let Some(&syn::TokenTree::Delimited(ref delimited)) = mac.tts.iter().next() {
                if let Some(&syn::TokenTree::Token(ref token)) = delimited.tts.iter().next() {
                    if let &syn::Token::Ident(ref ident) = token {
                        self.cvars.push(ident.as_ref().to_owned());
                    }
                }
            }
        }

        syn::visit::walk_mac(self, mac);
    }
}
