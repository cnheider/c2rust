#![feature(rustc_private)]
extern crate getopts;
extern crate idiomize;
extern crate syntax;

use std::collections::HashMap;
use std::str::FromStr;
use syntax::ast::NodeId;

use idiomize::{file_rewrite, driver, transform, span_fix, rewrite, pick_node};



struct Cursor {
    file: String,
    line: u32,
    col: u32,
    label: Option<String>,
    kind: Option<String>,
}

struct Mark {
    id: usize,
    label: Option<String>,
}

struct Options {
    rewrite_mode: file_rewrite::RewriteMode,
    command: String,
    command_args: Vec<String>,
    rustc_args: Vec<String>,
    cursors: Vec<Cursor>,
    marks: Vec<Mark>,
}

fn find<T: PartialEq<U>, U: ?Sized>(xs: &[T], x: &U) -> Option<usize> {
    for i in 0 .. xs.len() {
        if &xs[i] == x {
            return Some(i);
        }
    }
    None
}

fn print_usage(prog: &str, opts: &[getopts::OptGroup]) {
    let brief = format!("Usage: {} [options] transform [args...] -- [rustc args...]", prog);
    print!("{}", getopts::usage(&brief, opts));
}

fn parse_opts(argv: Vec<String>) -> Option<Options> {
    use getopts::{opt, HasArg, Occur};
    let opts = &[
        opt("r", "rewrite-mode",
            "output rewritten code `inplace`, `alongside` the original, \
               or `print` to screen? (default: print)",
            "MODE", HasArg::Yes, Occur::Optional),
        opt("c", "cursor",
            "a cursor position, used to filter some rewrite operations",
            "FILE:LINE:COL[:LABEL[:KIND]]", HasArg::Yes, Occur::Multi),
        opt("m", "mark",
            "a marked node indicated by its ID, and a label for that mark",
            "ID[:LABEL]", HasArg::Yes, Occur::Multi),
        opt("h", "help",
            "display usage information",
            "", HasArg::No, Occur::Optional),
    ];


    // Separate idiomize args from rustc args
    let (local_args, mut rustc_args) = match find(&argv, "--") {
        Some(idx) => {
            let mut argv = argv;
            let rest = argv.split_off(idx);
            (argv, rest)
        },
        None => {
            println!("Expected `--` followed by rustc arguments");
            print_usage(&argv[0], opts);
            return None;
        },
    };

    // Replace "--" with the program name
    rustc_args[0] = "rustc".to_owned();


    // Parse idiomize args
    let prog = &local_args[0];

    let m = match getopts::getopts(&local_args[1..], opts) {
        Ok(m) => m,
        Err(e) => {
            println!("{}", e.to_string());
            return None;
        },
    };

    if m.opt_present("h") {
        print_usage(prog, opts);
        return None;
    }

    // Parse rewrite mode
    let rewrite_mode = match m.opt_str("rewrite-mode") {
        Some(mode_str) => match &mode_str as &str {
            "inplace" => file_rewrite::RewriteMode::InPlace,
            "alongside" => file_rewrite::RewriteMode::Alongside,
            "print" => file_rewrite::RewriteMode::Print,
            _ => {
                println!("Unknown rewrite mode: {}", mode_str);
                return None;
            },
        },
        None => file_rewrite::RewriteMode::Print,
    };

    // Parse cursors
    let cursor_strs = m.opt_strs("cursor");
    let mut cursors = Vec::with_capacity(cursor_strs.len());
    for s in &cursor_strs {
        let mut parts = s.split(':');

        let file = match parts.next() {
            Some(x) => x.to_owned(),
            None => {
                println!("Bad cursor string: {:?}", s);
                return None;
            },
        };

        let line = match parts.next().map(|s| u32::from_str(s).map_err(|_| s)) {
            Some(Ok(x)) => x,
            Some(Err(s)) => {
                println!("Bad cursor line number: {:?}", s);
                return None;
            },
            None => {
                println!("Bad cursor string: {:?}", s);
                return None;
            }
        };

        let col = match parts.next().map(|s| u32::from_str(s).map_err(|_| s)) {
            Some(Ok(x)) => x,
            Some(Err(s)) => {
                println!("Bad cursor column number: {:?}", s);
                return None;
            },
            None => {
                println!("Bad cursor string: {:?}", s);
                return None;
            }
        };

        let label = match parts.next() {
            Some(s) if s.len() > 0 => Some(s.to_owned()),
            _ => None,
        };

        let kind = parts.next().map(|s| s.to_owned());

        if parts.next().is_some() {
            println!("Bad cursor string: {:?}", s);
            return None;
        }


        cursors.push(Cursor {
            file: file,
            line: line,
            col: col,
            label: label,
            kind: kind,
        });
    }

    // Parse marks
    let mark_strs = m.opt_strs("mark");
    let mut marks = Vec::with_capacity(mark_strs.len());
    for s in &mark_strs {
        let mut parts = s.split(':');

        let id = match parts.next().map(|s| usize::from_str(s).map_err(|_| s)) {
            Some(Ok(x)) => x,
            Some(Err(s)) => {
                println!("Bad mark node ID: {:?}", s);
                return None;
            },
            None => {
                println!("Bad mark string: {:?}", s);
                return None;
            }
        };

        let label = parts.next().map(|s| s.to_owned());

        if parts.next().is_some() {
            println!("Bad mark string: {:?}", s);
            return None;
        }


        marks.push(Mark {
            id: id,
            label: label,
        });
    }

    // Parse transform name + args
    if m.free.len() < 1 {
        println!("Missing transform name");
        return None;
    }
    let mut iter = m.free.clone().into_iter();
    let command = iter.next().unwrap();
    let command_args = iter.collect();

    Some(Options {
        rewrite_mode,
        command,
        command_args,
        rustc_args,
        cursors,
        marks,
    })
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let opts = match parse_opts(args) {
        Some(x) => x,
        None => return,
    };

    let mut marks = HashMap::new();
    for m in &opts.marks {
        marks.insert(NodeId::new(m.id), m.label.as_ref().map_or("target", |s| s).to_owned());
    }

    if opts.cursors.len() > 0 {
        driver::with_crate_and_context(&opts.rustc_args, driver::Phase::Phase2, |krate, cx| {
            for c in &opts.cursors {
                let kind_result = c.kind.clone().map_or(Ok(pick_node::NodeKind::Any),
                                                        |s| pick_node::NodeKind::from_str(&s));
                let kind = match kind_result {
                    Ok(k) => k,
                    Err(_) => {
                        println!("Bad cursor kind: {:?}", c.kind.as_ref().unwrap());
                        continue;
                    },
                };

                let id = match pick_node::pick_node_at_loc(
                        &krate, &cx, kind, &c.file, c.line, c.col) {
                    Some(info) => info.id,
                    None => {
                        println!("Failed to find {:?} at {}:{}:{}",
                                 kind, c.file, c.line, c.col);
                        continue;
                    },
                };

                let label = c.label.as_ref().map_or("target", |s| s).to_owned();

                println!("label {:?} as {:?}", id, label);

                marks.insert(id, label);
            }
        });
    }

    let opt_transform = transform::get_transform(&opts.command, &opts.command_args);
    if let Some(transform) = opt_transform {
        driver::with_crate_and_context(&opts.rustc_args, transform.min_phase(), |krate, mut cx| {
            cx.set_marks(marks);

            let krate = span_fix::fix_spans(cx.session(), krate);
            let krate2 = transform.transform(krate.clone(), &cx);

            let rws = rewrite::rewrite(cx.session(), &krate, &krate2);
            if rws.len() == 0 {
                println!("(no files to rewrite)");
            } else {
                file_rewrite::rewrite_files(cx.session().codemap(), &rws, opts.rewrite_mode);
            }
        });
    } else if &opts.command == "pick_node" {
        driver::with_crate_and_context(&opts.rustc_args, driver::Phase::Phase2, |krate, cx| {
            let krate = span_fix::fix_spans(cx.session(), krate);
            idiomize::pick_node::pick_node_command(&krate, &cx, &opts.command_args);
        });
    } else {
        panic!("unknown command: {:?}", opts.command);
    }
}
