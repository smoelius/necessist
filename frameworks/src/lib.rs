use anyhow::Result;
use clap::ValueEnum;
use heck::ToKebabCase;
use necessist_core::{
    framework::{
        Applicable, AsParse, AsRun, Interface, Parse as ParseHigh, Postprocess, Run as RunHigh,
        ToImplementation,
    },
    LightContext, SourceFile, Span, __Rewriter as Rewriter,
};
use std::{cell::RefCell, path::Path, rc::Rc};
use strum_macros::EnumIter;
use subprocess::Exec;

// Framework modules

mod anchor_ts;
use anchor_ts::AnchorTs;

mod foundry;
use foundry::Foundry;

mod go;
use go::Go;

mod hardhat_ts;
use hardhat_ts::HardhatTs;

mod rust;
use rust::Rust;

// Other modules

mod parsing;
use parsing::{AbstractTypes, MaybeNamed, Named, ParseAdapter, ParseLow, Spanned, WalkDirResult};

mod generic_visitor;
use generic_visitor::GenericVisitor;

mod running;
use running::{ProcessLines, RunAdapter, RunLow};

mod ts;

mod utils;
use utils::{OutputAccessors, OutputStrippedOfAnsiScapes};

#[derive(Debug, Clone, Copy, EnumIter, Eq, Ord, PartialEq, PartialOrd, ValueEnum)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[non_exhaustive]
#[remain::sorted]
pub enum Identifier {
    AnchorTs,
    Foundry,
    Go,
    HardhatTs,
    Rust,
}

impl Applicable for Identifier {
    fn applicable(&self, context: &LightContext) -> Result<bool> {
        match *self {
            Self::AnchorTs => AnchorTs::applicable(context),
            Self::Foundry => Foundry::applicable(context),
            Self::Go => Go::applicable(context),
            Self::HardhatTs => HardhatTs::applicable(context),
            Self::Rust => Rust::applicable(context),
        }
    }
}

impl ToImplementation for Identifier {
    // smoelius: `AnchorTs` and `HardhatTs` implement the `ParseLow` interface indirectly through
    // `ts::Mocha`. They implement the high-level `Run` interface directly.
    fn to_implementation(&self, context: &LightContext) -> Result<Option<Box<dyn Interface>>> {
        match *self {
            Self::AnchorTs => {
                let anchor = AnchorTs::new(context)?;
                Ok(Some(Box::new(anchor)))
            }

            Self::Foundry => Ok(Some(implementation_as_interface(ParseRunAdapter::new)(
                Foundry::new(),
            ))),

            Self::Go => Ok(Some(implementation_as_interface(ParseRunAdapter::new)(
                Go::new(),
            ))),

            Self::HardhatTs => Ok(Some(Box::new(HardhatTs::new()))),

            Self::Rust => Ok(Some(implementation_as_interface(ParseRunAdapter::new)(
                Rust::new(),
            ))),
        }
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("{self:?}").to_kebab_case())
    }
}

/// Utility function
fn implementation_as_interface<T, U: Interface + 'static>(
    adapter: impl Fn(T) -> U,
) -> impl Fn(T) -> Box<dyn Interface> {
    move |implementation| Box::new(adapter(implementation)) as Box<dyn Interface>
}

impl<T: RunHigh> RunHigh for ParseAdapter<T> {
    fn dry_run(&self, context: &LightContext, source_file: &Path) -> Result<()> {
        self.0.dry_run(context, source_file)
    }
    fn instrument_source_file(
        &self,
        context: &LightContext,
        rewriter: &mut Rewriter,
        source_file: &SourceFile,
        n_instrumentable_statements: usize,
    ) -> Result<()> {
        self.0
            .instrument_source_file(context, rewriter, source_file, n_instrumentable_statements)
    }
    fn statement_prefix_and_suffix(&self, span: &Span) -> Result<(String, String)> {
        self.0.statement_prefix_and_suffix(span)
    }
    fn build_source_file(&self, context: &LightContext, source_file: &Path) -> Result<()> {
        self.0.build_source_file(context, source_file)
    }
    fn exec(
        &self,
        context: &LightContext,
        test_name: &str,
        span: &Span,
    ) -> Result<Option<(Exec, Option<Box<Postprocess>>)>> {
        self.0.exec(context, test_name, span)
    }
}

impl<T: ParseLow + RunHigh> Interface for ParseAdapter<T> {}

struct ParseRunAdapter<T> {
    parse: ParseAdapter<Rc<RefCell<T>>>,
    run: RunAdapter<Rc<RefCell<T>>>,
}

impl<T> ParseRunAdapter<T> {
    fn new(value: T) -> Self {
        let rc = Rc::new(RefCell::new(value));
        Self {
            parse: ParseAdapter(rc.clone()),
            run: RunAdapter(rc),
        }
    }
}

impl<T: ParseLow> AsParse for ParseRunAdapter<T> {
    fn as_parse(&self) -> &dyn ParseHigh {
        &self.parse
    }
    fn as_parse_mut(&mut self) -> &mut dyn ParseHigh {
        &mut self.parse
    }
}

impl<T: RunLow> AsRun for ParseRunAdapter<T> {
    fn as_run(&self) -> &dyn RunHigh {
        &self.run
    }
}

impl<T: ParseLow + RunLow> Interface for ParseRunAdapter<T> {}
