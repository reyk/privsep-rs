use derive_more::From;
use slog::{Drain, Level, OwnedKVList, Record};
use std::{env, str::FromStr};

#[derive(From, Debug)]
struct Filter {
    module: Option<String>,
    level: Level,
}

impl Filter {
    #[inline]
    pub fn match_module(&self, module: &str) -> Option<&Self> {
        self.module.as_ref().map_or(Some(self), |prefix| {
            module.starts_with(prefix).then(|| self)
        })
    }

    #[inline]
    pub fn match_level(&self, level: Level) -> bool {
        level <= self.level
    }
}

struct Directives(Vec<Filter>);

impl Directives {
    #[inline]
    pub fn is_enabled(&self, module: &str, level: Level) -> bool {
        // Find the last-match filter and check the allowed level
        self.0
            .iter()
            .filter_map(|filter| filter.match_module(module))
            .last()
            .map(|filter| filter.match_level(level))
            .unwrap_or_default()
    }
}

/// Parse filter to be a list of valid prefix strings.
///
/// `module=level` or `level` where the module is a valid module
/// prefix and the level a supported level name (`critical`, `error`,
/// `warning`, `info`, `debug`, `trace`).
///
/// This method does not fail as it will ignore invalid directives.
impl From<String> for Directives {
    fn from(filter: String) -> Self {
        let filters = filter
            .split(',')
            .filter_map(|filter| {
                let kv = filter.split('=').collect::<Vec<_>>();
                if kv.len() == 1 {
                    Level::from_str(kv[0]).ok().map(|value| (None, value))
                } else if kv.len() == 2 {
                    let key = kv[0]
                        .chars()
                        .all(|c| matches!(c, '0'..='9' | 'a'..='z' | 'A'..='Z' | ':' | '_'))
                        .then(|| kv[0].to_string());
                    key.and_then(|key| Level::from_str(kv[1]).ok().map(|value| (Some(key), value)))
                } else {
                    None
                }
            })
            .map(Into::into)
            .collect();

        Self(filters)
    }
}

pub struct Logger<T: Drain> {
    drain: T,
    directives: Directives,
}

impl<T: Drain> Logger<T> {
    #[allow(unused)]
    pub fn new(drain: T) -> Self {
        Self::with_default_filter(drain, "info")
    }

    pub fn with_default_filter(drain: T, filter: &str) -> Self {
        let filter = env::var("RUST_LOG")
            .ok()
            .unwrap_or_else(|| filter.to_string());

        Self {
            drain,
            directives: filter.into(),
        }
    }
}

impl<T: Drain> Drain for Logger<T>
where
    T: Drain<Ok = ()>,
{
    type Err = T::Err;
    type Ok = ();

    fn log(&self, info: &Record<'_>, val: &OwnedKVList) -> Result<(), T::Err> {
        if !self.directives.is_enabled(info.module(), info.level()) {
            return Ok(());
        }

        self.drain.log(info, val)
    }
}
