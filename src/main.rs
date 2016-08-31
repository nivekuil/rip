// -*- compile-command: "cargo rustc" -*-
#[macro_use]
extern crate clap;

use clap::{Arg, App};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs::create_dir_all;
use std::env::current_dir;

static GRAVEYARD: &'static str = "/tmp/.graveyard";

fn main() {
    let matches = App::with_defaults("rip")
        .version(crate_version!())
        .author(crate_authors!())
        .about("Rm ImProved
Send files to the graveyard (/tmp/.graveyard) instead of unlinking them.")
        .arg(Arg::with_name("TARGET")
            .help("File or directory to remove")
            .required(true)
            .multiple(true)
            .index(1))
        .arg(Arg::with_name("graveyard")
            .help("Directory where deleted files go to rest")
            .long("graveyard")
            .takes_value(true))
        .get_matches();

    let graveyard: &Path = Path::new(matches.value_of("graveyard")
        .unwrap_or(GRAVEYARD));
    let sources: clap::Values = matches.values_of("TARGET").unwrap();

    for source in sources {
        send_to_graveyard(source, graveyard);
    }
}

fn send_to_graveyard(source: &str, graveyard: &Path) {
    let mut fullpath: PathBuf = current_dir().expect("Does current dir exist?");
    fullpath.push(Path::new(source));
    let parent: &Path = fullpath.parent().expect("Trying to delete / ?");
    let dest: PathBuf = graveyard.join(parent.strip_prefix("/").unwrap());

    create_dir_all(&dest).expect("Failed to create graveyard");

    let output = Command::new("mv")
        .arg("--backup=t")
        .arg(source)
        .arg(dest)
        .output()
        .expect("mv failed");

    if !output.status.success() {
        print!("{}", String::from_utf8(output.stderr).expect("Shell error"));
    }
}
