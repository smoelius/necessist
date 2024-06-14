use super::{
    AbstractTypes, GenericVisitor, MaybeNamed, Named, ParseLow, ProcessLines, RunLow, Spanned,
    WalkDirResult,
};
use anyhow::Result;
use cargo_metadata::Package;
use necessist_core::{
    framework::TestSpanMaps, LightContext, SourceFile, Span, ToInternalSpan, __Rewriter as Rewriter,
};
use once_cell::sync::Lazy;
use quote::ToTokens;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fs::read_to_string,
    path::{Path, PathBuf},
    process::Command,
    sync::RwLock,
};

mod storage;
use storage::{cached_test_file_package, Storage};

mod try_insert;
use try_insert::TryInsert;

mod visitor;
use visitor::visit;

#[derive(Debug)]
pub struct Rust {
    test_file_flags_cache: BTreeMap<PathBuf, Vec<String>>,
}

impl Rust {
    pub fn applicable(context: &LightContext) -> Result<bool> {
        context
            .root
            .join("Cargo.toml")
            .try_exists()
            .map_err(Into::into)
    }

    pub fn new() -> Self {
        Self {
            test_file_flags_cache: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Test<'ast> {
    test_path_id: usize,
    item_fn: &'ast syn::ItemFn,
}

impl<'ast> Test<'ast> {
    fn new(
        storage: &RefCell<Storage>,
        source_file: &SourceFile,
        item_fn: &'ast syn::ItemFn,
    ) -> Option<Self> {
        // smoelius: If the module path cannot be determined, return `None` to prevent the
        // `GenericVisitor` from walking the test.
        let test_name = item_fn.sig.ident.to_string();
        let result = storage.borrow_mut().test_path(source_file, &test_name);
        let test_path = match result {
            Ok(test_path) => test_path,
            Err(error) => {
                storage
                    .borrow_mut()
                    .tests_needing_warnings
                    .push((test_name, error));
                return None;
            }
        };
        let test_path_id = reserve_test_path_id(test_path);
        Some(Self {
            test_path_id,
            item_fn,
        })
    }
}

// smoelius: `TEST_PATH_ID_MAP` and `TEST_PATHS` cannot go in `Storage` because they are used by
// `Test`'s implementation of `Named`.
static TEST_PATH_ID_MAP: RwLock<Lazy<HashMap<Vec<String>, usize>>> =
    RwLock::new(Lazy::new(HashMap::new));
static TEST_PATHS: RwLock<Vec<Vec<String>>> = RwLock::new(Vec::new());

fn reserve_test_path_id(test_path: Vec<String>) -> usize {
    let test_path_id_map = TEST_PATH_ID_MAP.read().unwrap();
    let test_path_id = test_path_id_map.get(&test_path).copied();
    drop(test_path_id_map);
    if let Some(test_path_id) = test_path_id {
        test_path_id
    } else {
        let mut test_paths = TEST_PATHS.write().unwrap();
        test_paths.push(test_path);
        test_paths.len() - 1
    }
}

#[derive(Clone, Copy)]
pub enum Expression<'ast> {
    Await(&'ast syn::ExprAwait),
    Field(Field<'ast>),
    Call(Call<'ast>),
    MacroCall(MacroCall<'ast>),
    Other(proc_macro2::Span),
}

impl<'ast> From<&'ast syn::Expr> for Expression<'ast> {
    fn from(value: &'ast syn::Expr) -> Self {
        match value {
            syn::Expr::Await(await_) => Expression::Await(await_),
            syn::Expr::Field(field) => Expression::Field(Field::Field(field)),
            syn::Expr::Call(call) => Expression::Call(Call::FunctionCall(call)),
            syn::Expr::Macro(mac) => Expression::MacroCall(MacroCall::Expr(mac)),
            syn::Expr::MethodCall(method_call) => Expression::Call(Call::MethodCall(method_call)),
            _ => Expression::Other(<_ as syn::spanned::Spanned>::span(value)),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Field<'ast> {
    Field(&'ast syn::ExprField),

    /// A method call pretending to be a field.
    MethodCall(&'ast syn::ExprMethodCall),
}

#[derive(Clone, Copy)]
pub enum Call<'ast> {
    FunctionCall(&'ast syn::ExprCall),
    MethodCall(&'ast syn::ExprMethodCall),
}

#[derive(Clone, Copy)]
pub enum MacroCall<'ast> {
    Stmt(&'ast syn::StmtMacro),
    Expr(&'ast syn::ExprMacro),
}

impl<'ast> MacroCall<'ast> {
    fn path(&self) -> &syn::Path {
        match self {
            MacroCall::Stmt(syn::StmtMacro { mac, .. })
            | MacroCall::Expr(syn::ExprMacro { mac, .. }) => &mac.path,
        }
    }
}

pub struct Types;

impl AbstractTypes for Types {
    type Storage<'ast> = Storage<'ast>;
    type File = syn::File;
    type Test<'ast> = Test<'ast>;
    type Statement<'ast> = &'ast syn::Stmt;
    type Expression<'ast> = Expression<'ast>;
    type Await<'ast> = &'ast syn::ExprAwait;
    type Field<'ast> = Field<'ast>;
    type Call<'ast> = Call<'ast>;
    type MacroCall<'ast> = MacroCall<'ast>;
}

// smoelius: See note above re `TEST_PATH_ID_MAP` and `TEST_PATHS`.
impl<'ast> Named for Test<'ast> {
    fn name(&self) -> String {
        let test_paths = TEST_PATHS.read().unwrap();
        test_paths[self.test_path_id].join("::")
    }
}

// smoelius: Implementing `MaybeNamed` for `Expression` is mainly to deal with languages where
// calling a function in a module/package looks like a field access, e.g., Go and TS. Since Rust is
// not such a language, it is safe to return `None`.
impl<'ast> MaybeNamed for Expression<'ast> {
    fn name(&self) -> Option<String> {
        None
    }
}

impl<'ast> MaybeNamed for <Types as AbstractTypes>::Field<'ast> {
    fn name(&self) -> Option<String> {
        match self {
            Field::Field(field) => {
                if let syn::Member::Named(ident) = &field.member {
                    Some(ident.to_string())
                } else {
                    None
                }
            }
            Field::MethodCall(method_call) => Some(method_call.method.to_string()),
        }
    }
}

impl<'ast> MaybeNamed for <Types as AbstractTypes>::Call<'ast> {
    fn name(&self) -> Option<String> {
        match self {
            Call::FunctionCall(call) => {
                if let syn::Expr::Path(path) = &*call.func {
                    Some(
                        path.to_token_stream()
                            .into_iter()
                            .map(|tt| tt.to_string())
                            .collect::<String>(),
                    )
                } else {
                    None
                }
            }
            Call::MethodCall(method_call) => Some(method_call.method.to_string()),
        }
    }
}

impl<'ast> Named for <Types as AbstractTypes>::MacroCall<'ast> {
    fn name(&self) -> String {
        self.path().to_token_stream().to_string().replace(' ', "")
    }
}

impl<'ast> Spanned for <Types as AbstractTypes>::Expression<'ast> {
    fn span(&self, source_file: &SourceFile) -> Span {
        match self {
            Expression::Await(await_) => {
                <_ as syn::spanned::Spanned>::span(await_).to_internal_span(source_file)
            }
            Expression::Call(call) => call.span(source_file),
            Expression::Field(field) => field.span(source_file),
            Expression::MacroCall(macro_call) => macro_call.span(source_file),
            Expression::Other(span) => span.to_internal_span(source_file),
        }
    }
}

impl<'ast> Spanned for <Types as AbstractTypes>::Statement<'ast> {
    fn span(&self, source_file: &SourceFile) -> Span {
        <_ as syn::spanned::Spanned>::span(self).to_internal_span(source_file)
    }
}

impl<'ast> Spanned for <Types as AbstractTypes>::Field<'ast> {
    fn span(&self, source_file: &SourceFile) -> Span {
        match self {
            Field::Field(field) => {
                <_ as syn::spanned::Spanned>::span(field).to_internal_span(source_file)
            }
            Field::MethodCall(method_call) => {
                <_ as syn::spanned::Spanned>::span(method_call).to_internal_span(source_file)
            }
        }
    }
}

impl<'ast> Spanned for <Types as AbstractTypes>::Call<'ast> {
    fn span(&self, source_file: &SourceFile) -> Span {
        match self {
            Call::FunctionCall(call) => {
                <_ as syn::spanned::Spanned>::span(call).to_internal_span(source_file)
            }
            Call::MethodCall(method_call) => {
                <_ as syn::spanned::Spanned>::span(method_call).to_internal_span(source_file)
            }
        }
    }
}

impl<'ast> Spanned for <Types as AbstractTypes>::MacroCall<'ast> {
    fn span(&self, source_file: &SourceFile) -> Span {
        match self {
            MacroCall::Stmt(stmt) => {
                <_ as syn::spanned::Spanned>::span(stmt).to_internal_span(source_file)
            }
            MacroCall::Expr(expr) => {
                <_ as syn::spanned::Spanned>::span(expr).to_internal_span(source_file)
            }
        }
    }
}

impl ParseLow for Rust {
    type Types = Types;

    const IGNORED_FUNCTIONS: Option<&'static [&'static str]> = Some(&[]);

    const IGNORED_MACROS: Option<&'static [&'static str]> = Some(&[
        "assert",
        "assert_eq",
        "assert_matches",
        "assert_ne",
        "eprint",
        "eprintln",
        "panic",
        "print",
        "println",
        "unimplemented",
        "unreachable",
    ]);

    const IGNORED_METHODS: Option<&'static [&'static str]> = Some(&[
        "as_bytes",
        "as_encoded_bytes",
        "as_mut",
        "as_mut_os_str",
        "as_mut_os_string",
        "as_mut_slice",
        "as_mut_str",
        "as_os_str",
        "as_path",
        "as_ref",
        "as_slice",
        "as_str",
        "borrow",
        "borrow_mut",
        "clone",
        "cloned",
        "copied",
        "deref",
        "deref_mut",
        "expect",
        "expect_err",
        "into_boxed_bytes",
        "into_boxed_os_str",
        "into_boxed_path",
        "into_boxed_slice",
        "into_boxed_str",
        "into_bytes",
        "into_encoded_bytes",
        "into_os_string",
        "into_owned",
        "into_path_buf",
        "into_string",
        "into_vec",
        "iter",
        "iter_mut",
        "success",
        "to_os_string",
        "to_owned",
        "to_path_buf",
        "to_string",
        "to_vec",
        "unwrap",
        "unwrap_err",
    ]);

    fn walk_dir(&self, root: &Path) -> Box<dyn Iterator<Item = WalkDirResult>> {
        Box::new(
            walkdir::WalkDir::new(root)
                .into_iter()
                .filter_entry(|entry| {
                    let path = entry.path();
                    path.file_name() != Some(OsStr::new("target"))
                        && (!path.is_file() || path.extension() == Some(OsStr::new("rs")))
                }),
        )
    }

    fn parse_file(&self, test_file: &Path) -> Result<<Self::Types as AbstractTypes>::File> {
        let content = read_to_string(test_file)?;
        syn::parse_file(&content).map_err(Into::into)
    }

    fn storage_from_file<'ast>(
        &self,
        file: &'ast <Self::Types as AbstractTypes>::File,
    ) -> <Self::Types as AbstractTypes>::Storage<'ast> {
        Storage::new(file)
    }

    fn visit_file<'ast>(
        generic_visitor: GenericVisitor<'_, '_, '_, 'ast, Self>,
        storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        file: &'ast <Self::Types as AbstractTypes>::File,
    ) -> Result<TestSpanMaps> {
        visit(generic_visitor, storage, file)
    }

    fn test_statements<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        test: <Self::Types as AbstractTypes>::Test<'ast>,
    ) -> Vec<<Self::Types as AbstractTypes>::Statement<'ast>> {
        test.item_fn
            .block
            .stmts
            .iter()
            .map(|stmt| stmt as _)
            .collect::<Vec<_>>()
    }

    fn statement_is_expression<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        statement: <Self::Types as AbstractTypes>::Statement<'ast>,
    ) -> Option<<Self::Types as AbstractTypes>::Expression<'ast>> {
        match statement {
            syn::Stmt::Expr(expr, _) => Some(Expression::from(expr)),
            syn::Stmt::Macro(mac) => Some(Expression::MacroCall(MacroCall::Stmt(mac))),
            _ => None,
        }
    }

    fn statement_is_control<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        statement: <Self::Types as AbstractTypes>::Statement<'ast>,
    ) -> bool {
        match statement {
            syn::Stmt::Expr(expr, _) => Some(expr),
            _ => None,
        }
        .map_or(false, |expr| {
            matches!(
                expr,
                syn::Expr::Break(_) | syn::Expr::Continue(_) | syn::Expr::Return(_)
            )
        })
    }

    fn statement_is_declaration<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        statement: <Self::Types as AbstractTypes>::Statement<'ast>,
    ) -> bool {
        matches!(statement, syn::Stmt::Item(_) | syn::Stmt::Local(_))
    }

    fn expression_is_await<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        expression: <Self::Types as AbstractTypes>::Expression<'ast>,
    ) -> Option<<Self::Types as AbstractTypes>::Await<'ast>> {
        if let Expression::Await(await_) = expression {
            Some(await_)
        } else {
            None
        }
    }

    fn expression_is_field<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        expression: <Self::Types as AbstractTypes>::Expression<'ast>,
    ) -> Option<<Self::Types as AbstractTypes>::Field<'ast>> {
        if let Expression::Field(field) = expression {
            Some(field)
        } else {
            None
        }
    }

    fn expression_is_call<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        expression: <Self::Types as AbstractTypes>::Expression<'ast>,
    ) -> Option<<Self::Types as AbstractTypes>::Call<'ast>> {
        if let Expression::Call(call) = expression {
            Some(call)
        } else {
            None
        }
    }

    fn expression_is_macro_call<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        expression: <Self::Types as AbstractTypes>::Expression<'ast>,
    ) -> Option<<Self::Types as AbstractTypes>::MacroCall<'ast>> {
        if let Expression::MacroCall(macro_call) = expression {
            Some(macro_call)
        } else {
            None
        }
    }

    fn await_arg<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        await_: <Self::Types as AbstractTypes>::Await<'ast>,
    ) -> <Self::Types as AbstractTypes>::Expression<'ast> {
        Expression::from(&*await_.base)
    }

    fn field_base<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        field: <Self::Types as AbstractTypes>::Field<'ast>,
    ) -> <Self::Types as AbstractTypes>::Expression<'ast> {
        Expression::from(match field {
            Field::Field(field) => &*field.base,
            Field::MethodCall(method_call) => &*method_call.receiver,
        })
    }

    fn call_callee<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        call: <Self::Types as AbstractTypes>::Call<'ast>,
    ) -> <Self::Types as AbstractTypes>::Expression<'ast> {
        match call {
            Call::FunctionCall(call) => Expression::from(&*call.func),
            Call::MethodCall(method_call) => Expression::Field(Field::MethodCall(method_call)),
        }
    }

    fn macro_call_callee<'ast>(
        &self,
        _storage: &RefCell<<Self::Types as AbstractTypes>::Storage<'ast>>,
        macro_call: <Self::Types as AbstractTypes>::MacroCall<'ast>,
    ) -> <Self::Types as AbstractTypes>::Expression<'ast> {
        let mac = match macro_call {
            MacroCall::Stmt(mac) => &mac.mac,
            MacroCall::Expr(mac) => &mac.mac,
        };
        let span_path = <_ as syn::spanned::Spanned>::span(&mac.path);
        let span_bang = <_ as syn::spanned::Spanned>::span(&mac.bang_token);
        Expression::Other(span_path.join(span_bang).unwrap())
    }
}

impl RunLow for Rust {
    fn command_to_run_test_file(&self, context: &LightContext, test_file: &Path) -> Command {
        self.test_command(context, test_file)
    }

    fn instrument_file(
        &self,
        _context: &LightContext,
        _rewriter: &mut Rewriter,
        _source_file: &SourceFile,
        _n_instrumentable_statements: usize,
    ) -> Result<()> {
        Ok(())
    }

    fn statement_prefix_and_suffix(&self, span: &Span) -> Result<(String, String)> {
        Ok((
            format!(
                r#"if std::env::var("NECESSIST_REMOVAL").unwrap() != "{}" {{ "#,
                span.id()
            ),
            " }".to_owned(),
        ))
    }

    fn command_to_build_file(&self, context: &LightContext, source_file: &Path) -> Command {
        let mut command = self.test_command(context, source_file);
        command.arg("--no-run");
        command
    }

    fn command_to_build_test(
        &self,
        context: &LightContext,
        _test_name: &str,
        span: &Span,
    ) -> Command {
        let mut command = self.test_command(context, &span.source_file);
        command.arg("--no-run");
        command
    }

    fn command_to_run_test(
        &self,
        context: &LightContext,
        test_name: &str,
        span: &Span,
    ) -> (Command, Vec<String>, Option<ProcessLines>) {
        (
            self.test_command(context, &span.source_file),
            vec!["--".to_owned(), "--exact".to_owned(), test_name.to_owned()],
            Some((false, Box::new(|line| line == "running 1 test"))),
        )
    }
}

impl Rust {
    fn test_command(&self, _context: &LightContext, test_file: &Path) -> Command {
        #[allow(clippy::expect_used)]
        let flags = self
            .test_file_flags_cache
            .get(test_file)
            .expect("Flags are not cached");
        let mut command = Command::new("cargo");
        command.arg("test");
        command.args(flags);
        command
    }

    #[cfg_attr(dylint_lib = "general", allow(non_local_effect_before_error_return))]
    fn cached_test_file_flags(
        &mut self,
        test_file_package_map: &mut BTreeMap<PathBuf, Package>,
        test_file: &Path,
    ) -> Result<&Vec<String>> {
        self.test_file_flags_cache
            .entry(test_file.to_path_buf())
            .or_try_insert_with(|| {
                let package = cached_test_file_package(test_file_package_map, test_file)?;

                let mut flags = vec![
                    "--manifest-path".to_owned(),
                    package.manifest_path.as_str().to_owned(),
                ];

                if let Some(name) = test_file_test(package, test_file) {
                    flags.extend(["--test".to_owned(), name.clone()]);
                } else {
                    // smoelius: Failed to find a test target with this file name. Assume it is a
                    // unit test.
                    let mut bin = false;
                    let mut lib = false;
                    for kind in package.targets.iter().flat_map(|target| &target.kind) {
                        match kind.as_ref() {
                            "bin" if !bin => {
                                flags.push("--bins".to_owned());
                                bin = true;
                            }
                            "lib" if !lib => {
                                flags.push("--lib".to_owned());
                                lib = true;
                            }
                            _ => {}
                        }
                    }
                }

                Ok(flags)
            })
            .map(|value| value as &_)
    }
}

fn test_file_test<'a>(package: &'a Package, test_file: &Path) -> Option<&'a String> {
    if let &[name] = package
        .targets
        .iter()
        .filter_map(|target| {
            if target.kind == ["test"] && target.src_path == test_file {
                Some(&target.name)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .as_slice()
    {
        Some(name)
    } else {
        None
    }
}
