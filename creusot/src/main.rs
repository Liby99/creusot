#![feature(rustc_private, register_tool)]
#![feature(box_syntax, box_patterns, control_flow_enum)]
#![feature(in_band_lifetimes)]

#![register_tool(creusot)]
#![feature(const_panic)]

extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_index;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_mir;
extern crate rustc_resolve;
extern crate rustc_session;
extern crate rustc_span;
extern crate rustc_target;
extern crate rustc_trait_selection;

#[macro_use]
extern crate log;

use heck::CamelCase;
use rustc_driver::{abort_on_err, Callbacks, Compilation, RunCompiler};
use rustc_hir::{def_id::LOCAL_CRATE, Item};
use rustc_interface::{
    interface::{BoxedResolver, Compiler},
    Queries,
};
use rustc_middle::{
    mir::{visit::MutVisitor, Location, Terminator},
    ty::TyCtxt,
};
use std::{cell::RefCell, env::args as get_args, rc::Rc};
use why3::mlcfg;

pub mod util;
mod analysis;
pub mod ctx;
mod resolve;
mod extended_location;
mod translation;

#[allow(dead_code)]
mod debug;

use rustc_session::Session;
use translation::*;

mod modules;
mod rustc_extensions;

struct ToWhy {
    output_file: Option<String>,
}

impl Callbacks for ToWhy {
    fn after_expansion<'tcx>(&mut self, c: &Compiler, queries: &'tcx Queries<'tcx>) -> Compilation {
        let session = c.session();
        let resolver = {
            let (_, resolver, _) = &* abort_on_err(queries.expansion(), session).peek();
            resolver.clone()
        };

        queries.prepare_outputs().unwrap();

        queries.global_ctxt().unwrap();

        queries.global_ctxt().unwrap().peek_mut().enter(|tcx| tcx.analysis(())).unwrap();

        queries
            .global_ctxt()
            .unwrap()
            .peek_mut()
            .enter(|tcx| {
                let session = c.session();
                // TODO: Resolve extern crates
                translate(&self.output_file, session, tcx, resolver)
            })
            .unwrap();
        Compilation::Stop
    }
}

fn main() {
    env_logger::init();

    let mut args = get_args().collect::<Vec<String>>();

    let output_file = args.iter().position(|a| a == "-o").map(|ix| args[ix + 1].clone());

    args.push(format!("--sysroot={}", sysroot_path()));
    args.push("-Cpanic=abort".to_owned());
    args.push("-Coverflow-checks=off".to_owned());
    // args.push("-Znll-facts".to_owned());
    RunCompiler::new(&args, &mut ToWhy { output_file }).run().unwrap();
}

use std::io::Result;

fn is_type_decl(item: &Item) -> bool {
    match item.kind {
        // rustc_hir::ItemKind::TyAlias(_, _) => true,
        rustc_hir::ItemKind::OpaqueTy(_) => unimplemented!(),
        rustc_hir::ItemKind::Enum(_, _) => true,
        rustc_hir::ItemKind::Struct(_, _) => true,
        rustc_hir::ItemKind::Union(_, _) => unimplemented!(),
        _ => false,
    }
}

fn translate(
    output: &Option<String>,
    sess: &Session,
    tcx: TyCtxt,
    resolver: Rc<RefCell<BoxedResolver>>,
) -> Result<()> {
    let hir_map = tcx.hir();

    // Collect the DefIds of all type declarations in this crate
    let mut ty_decls = Vec::new();
    log::debug!("translate");

    for (_, mod_items) in tcx.hir_crate(()).modules.iter() {
        for item_id in mod_items.items.iter() {
            let item = hir_map.item(*item_id);
            // What about inline type declarations?
            // How do we find those?
            if is_type_decl(item) {
                ty_decls.push((item.def_id.to_def_id(), item.span));
            }
        }
    }

    // Type translation state, including which datatypes have already been translated.
    let mut ty_ctx = ctx::TranslationCtx::new(tcx, sess, resolver.clone());

    // Translate all type declarations and push them into the module collection
    for (def_id, span) in ty_decls.iter() {
        debug!("Translating type declaration {:?}", def_id);
        translation::ty::translate_tydecl(&mut ty_ctx, *span, *def_id);
    }

    for def_id in tcx.body_owners() {
        debug!("Translating body {:?}", def_id);

        let def_id = def_id.to_def_id();
        ty_ctx.translate_function(def_id);
    }

    use std::fs::File;

    let mut out: Box<dyn Write> = match output {
        Some(f) => Box::new(std::io::BufWriter::new(File::create(f)?)),
        None => Box::new(std::io::stdout()),
    };

    print_crate(&mut out, tcx.crate_name(LOCAL_CRATE).to_string().to_camel_case(), ty_ctx.modules)?;
    Ok(())
}
use std::io::Write;

// TODO: integrate crate name into the modules
fn print_crate<W>(out: &mut W, _name: String, krate: modules::Modules) -> std::io::Result<()>
where
    W: Write,
{
    let (alloc, mut pe) = mlcfg::printer::PrintEnv::new();

    for modl in krate.into_iter() {
        modl.pretty(&alloc, &mut pe).1.render(120, out)?;
        writeln!(out)?;
    }

    Ok(())
}

fn sysroot_path() -> String {
    use std::process::Command;
    let toolchain: toml::Value = toml::from_str(include_str!("../../rust-toolchain")).unwrap();
    let channel = toolchain["toolchain"]["channel"].as_str().unwrap();

    let output = Command::new("rustup")
        .arg("run")
        .arg(channel)
        .arg("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()
        .unwrap();

    print!("{}", String::from_utf8(output.stderr).ok().unwrap());

    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct RemoveFalseEdge<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> MutVisitor<'tcx> for RemoveFalseEdge<'tcx> {
    fn tcx<'a>(&'a self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_terminator(&mut self, terminator: &mut Terminator<'tcx>, _location: Location) {
        if let rustc_middle::mir::TerminatorKind::FalseEdge { real_target, .. } = terminator.kind {
            terminator.kind = rustc_middle::mir::TerminatorKind::Goto { target: real_target }
        }
    }
}
