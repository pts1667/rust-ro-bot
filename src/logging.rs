//! Logging: everything to the terminal, or with `--log-dir` the events of each bot (those inside its `bot` span) to a
//! file of its own and the rest to the terminal.

use std::fmt::Debug;
use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Metadata, Subscriber};
use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};

/// The name of the span a bot's work runs in; its `name` field is the bot's.
pub const BOT_SPAN: &str = "bot";

/// Starts the logging, with a file `<dir>/<name>.log` for each of `bots` when `dir` is given.
pub fn init(dir: Option<&Path>, bots: &[String]) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let Some(dir) = dir else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
        return Ok(());
    };
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();
    for bot in bots {
        let path = dir.join(format!("{bot}.log"));
        let file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        let bot = bot.clone();
        layers.push(
            fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .with_filter(dynamic_filter_fn(move |meta, cx| {
                    // Every span passes, or the bot's own span would be missing from the lookup below
                    meta.is_span() || bot_is(cx, |b| b == Some(bot.as_str()))
                }))
                .boxed(),
        );
    }
    layers.push(
        fmt::layer()
            .with_filter(dynamic_filter_fn(|meta: &Metadata<'_>, cx| {
                meta.is_span() || bot_is(cx, |b| b.is_none())
            }))
            .boxed(),
    );
    tracing_subscriber::registry()
        .with(layers)
        .with(BotNames)
        .with(filter)
        .init();
    Ok(())
}

/// Whether `test` holds for the bot of the span the event comes in (`None` outside of every bot).
fn bot_is<S>(cx: &Context<'_, S>, test: impl Fn(Option<&str>) -> bool) -> bool
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let Some(span) = cx.lookup_current() else {
        return test(None);
    };
    for s in span.scope() {
        if let Some(b) = s.extensions().get::<BotName>() {
            return test(Some(&b.0));
        }
    }
    test(None)
}

/// The bot a span belongs to, kept by `BotNames`.
struct BotName(String);

/// Keeps the `name` of every bot span in its extensions, for the filters.
struct BotNames;

impl<S> Layer<S> for BotNames
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, cx: Context<'_, S>) {
        if attrs.metadata().name() != BOT_SPAN {
            return;
        }
        let mut name = NameField(None);
        attrs.record(&mut name);
        if let (Some(name), Some(span)) = (name.0, cx.span(id)) {
            span.extensions_mut().insert(BotName(name));
        }
    }
}

struct NameField(Option<String>);

impl Visit for NameField {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "name" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}
