extern crate proc_macro2;
extern crate syn;
extern crate walkdir;

use proc_macro2::TokenTree;
use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use syn::{
    visit::{self, Visit},
    Macro,
};

fn main() {
    // Parse all .rs files to collect everything which implements Command.
    // This code won't work properly with lib.rs or mod.rs.
    let mut data = Data::new();

    for entry in
        walkdir::WalkDir::new("src").into_iter()
                                    .filter_map(|e| e.ok())
                                    .filter(|e| !e.file_type().is_dir())
                                    .filter(|e| e.path().extension() == Some(OsStr::new("rs")))
    {
        let mut path = String::new();

        for name in entry.path().with_extension("").iter().skip(1) {
            path.push_str(&format!("::{}", name.to_str().unwrap()));
        }

        let entry_data = get_data(entry.path());

        data.commands.extend(entry_data.commands
                                       .into_iter()
                                       .map(|c| format!("{}::{}", &path, c)));
        data.cvars.extend(entry_data.cvars
                                    .into_iter()
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
}

struct Data {
    commands: Vec<String>,
    cvars: Vec<String>,
}

impl Data {
    fn new() -> Self {
        Self { commands: Vec::new(),
               cvars: Vec::new(), }
    }
}

fn get_data(path: &Path) -> Data {
    let mut source = String::new();
    File::open(path).unwrap()
                    .read_to_string(&mut source)
                    .unwrap();

    if let Ok(file) = syn::parse_file(&source) {
        let mut visitor = MyVisitor::new();
        visitor.visit_file(&file);

        Data { commands: visitor.commands,
               cvars: visitor.cvars, }
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
    let mut buf = format!("pub static CVARS: [&::cvar::CVar; {}] = [", cvars.len());

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
        Self { commands: Vec::new(),
               cvars: Vec::new(), }
    }
}

impl<'ast> Visit<'ast> for MyVisitor {
    fn visit_macro(&mut self, mac: &'ast Macro) {
        if mac.path
              .segments
              .first()
              .map(|x| x.value().ident == "command")
              .unwrap_or(false)
        {
            if let Some(TokenTree::Ident(ident)) = mac.tts.clone().into_iter().next() {
                self.commands.push(format!("{}", ident));
            }
        }

        if mac.path
              .segments
              .first()
              .map(|x| x.value().ident == "cvar")
              .unwrap_or(false)
        {
            if let Some(TokenTree::Ident(ident)) = mac.tts.clone().into_iter().next() {
                self.cvars.push(format!("{}", ident));
            }
        }

        visit::visit_macro(self, mac);
    }
}
