mod io_util;
mod tokenizer;
mod verifier;


use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::verifier::verify;


#[derive(Parser)]
struct Opts {
    /// Tokenize instead of verifying.
    #[arg(short, long)]
    pub tokenize: bool,

    /// The JSON file to verify.
    pub json_file: PathBuf,
}


fn main() -> ExitCode {
    let opts = Opts::parse();

    let file = File::open(&opts.json_file)
        .expect("failed to open JSON file");
    let mut reader = BufReader::new(file);

    if opts.tokenize {
        while let Some(tok) = crate::tokenizer::read_next_token(&mut reader).expect("failed to read") {
            println!("{:?}", tok);
        }
        ExitCode::SUCCESS
    } else {
        if verify(&mut reader) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}
