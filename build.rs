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
    let mut commands = Vec::new();

    for entry in walkdir::WalkDir::new("src")
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().is_dir()) {
        let mut path = String::new();

        for name in entry.path().with_extension("").iter().skip(1) {
            path.push_str(&format!("::{}", name.to_str().unwrap()));
        }

        commands.extend(get_commands(entry.path())
            .into_iter()
            .map(|c| format!("{}::{}", &path, c)))
    }

    let command_array = make_array(commands);

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("command_array.rs");
    let mut f = File::create(&dest_path).unwrap();
    write!(f, "{}", command_array).unwrap();
}

fn get_commands(path: &Path) -> Vec<String> {
    let mut source = String::new();
    File::open(path).unwrap().read_to_string(&mut source).unwrap();

    let _crate = syn::parse_crate(&source).expect("Error parsing the code");

    let mut visitor = CommandVisitor::new();
    visitor.visit_crate(&_crate);

    visitor.commands
}

fn make_array(commands: Vec<String>) -> String {
    let mut buf = format!("pub static COMMANDS: [&Command; {}] = [", commands.len());

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

struct CommandVisitor {
    commands: Vec<String>,
}

impl CommandVisitor {
    fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl Visitor for CommandVisitor {
    fn visit_mac(&mut self, mac: &syn::Mac) {
        println!("Visiting macro: {:?}", mac.path);
        if mac.path == "command".into() {
            if let Some(&syn::TokenTree::Delimited(ref delimited)) = mac.tts.iter().next() {
                if let Some(&syn::TokenTree::Token(ref token)) = delimited.tts.iter().next() {
                    if let &syn::Token::Ident(ref ident) = token {
                        self.commands.push(ident.as_ref().to_owned());
                    }
                }
            }
        }

        syn::visit::walk_mac(self, mac);
    }
}
