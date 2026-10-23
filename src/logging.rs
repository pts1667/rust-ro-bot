//! Logging: everything to the terminal, or with `--log-dir` the events of each bot (those inside its `bot` span) to a
//! file of its own and the rest to the terminal.

use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
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
    let mut files = HashMap::new();
    for bot in bots {
        let path = dir.join(format!("{bot}.log"));
        let file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        files.insert(
            bot.clone(),
            fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .boxed(),
        );
    }
    let router = PerBot {
        files,
        rest: fmt::layer().boxed(),
    };
    tracing_subscriber::registry()
        .with(router)
        .with(BotNames)
        .with(filter)
        .init();
    Ok(())
}

type Boxed = Box<dyn Layer<Registry> + Send + Sync>;

/// Writes each event to the log of the bot whose span it comes in, the others to `rest`. One layer routing, not a
/// filtered layer per bot: tracing-subscriber takes at most 64 per-layer filters, and 200 bots panicked at start.
struct PerBot {
    files: HashMap<String, Boxed>,
    rest: Boxed,
}

impl PerBot {
    /// The layer that formats span fields, kept in the span for every layer: one without colours, else the files
    /// get the terminal's escape codes.
    fn fields(&self) -> &Boxed {
        self.files.values().next().unwrap_or(&self.rest)
    }

    fn all(&mut self) -> impl Iterator<Item = &mut Boxed> {
        self.files.values_mut().chain(std::iter::once(&mut self.rest))
    }
}

impl Layer<Registry> for PerBot {
    fn on_register_dispatch(&self, subscriber: &tracing::Dispatch) {
        for layer in self.files.values().chain(std::iter::once(&self.rest)) {
            layer.on_register_dispatch(subscriber);
        }
    }

    fn on_layer(&mut self, subscriber: &mut Registry) {
        for layer in self.all() {
            layer.on_layer(subscriber);
        }
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, cx: Context<'_, Registry>) {
        self.fields().on_new_span(attrs, id, cx);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, cx: Context<'_, Registry>) {
        self.fields().on_record(id, values, cx);
    }

    fn on_close(&self, id: Id, cx: Context<'_, Registry>) {
        self.fields().on_close(id, cx);
    }

    fn on_event(&self, event: &Event<'_>, cx: Context<'_, Registry>) {
        let bot = cx.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find_map(|s| s.extensions().get::<BotName>().map(|b| b.0.clone()))
        });
        match bot.and_then(|b| self.files.get(&b)) {
            Some(layer) => layer.on_event(event, cx),
            None => self.rest.on_event(event, cx),
        }
    }
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
