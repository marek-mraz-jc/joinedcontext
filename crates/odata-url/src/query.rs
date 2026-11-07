// SPDX-License-Identifier: EUPL-1.2
//! `queryOptions` (ABNF section 2): system query options, parameter aliases and custom options.

use crate::cursor::{count, Cursor};
use crate::expr::{annotation, common_expr, Budget, Scope};
use crate::literal::{literal, pct_encoded};
use crate::model::{Member, Model};
use crate::search::search;
use crate::{
    Compute, ExpandItem, ExpandKind, ExpandPath, Feature, Format, Levels, LimitKind, OrderBy,
    QueryOptions, SelectItem, Transformation, UrlError, UrlErrorKind,
};

/// Which options a URL form allows besides custom ones (ABNF `batchOptions`,
/// `metadataOptions`, `entityOptions`, `queryOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Allowed {
    /// `$format` only.
    Format,
    /// `$format` and `$id`, which is required.
    EntityId,
    /// Every system option and aliases.
    All,
}

/// System query options by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Name {
    Filter,
    Select,
    Expand,
    Orderby,
    Top,
    Skip,
    Count,
    Format,
    Skiptoken,
    Schemaversion,
    Id,
    Search,
    Compute,
    Index,
    Deltatoken,
    Apply,
    Levels,
}

/// Names matched case-insensitively, with or without `$` (Protocol 4.01 11.2.1) unless the
/// flag says `$` is required (`$skiptoken`, `$deltatoken` in the ABNF; `$apply` in Data
/// Aggregation 4.0).
const NAMES: [(&str, bool, Name); 17] = [
    ("$filter", true, Name::Filter),
    ("$select", true, Name::Select),
    ("$expand", true, Name::Expand),
    ("$orderby", true, Name::Orderby),
    ("$top", true, Name::Top),
    ("$skiptoken", false, Name::Skiptoken),
    ("$skip", true, Name::Skip),
    ("$count", true, Name::Count),
    ("$format", true, Name::Format),
    ("$schemaversion", true, Name::Schemaversion),
    ("$id", true, Name::Id),
    ("$search", true, Name::Search),
    ("$compute", true, Name::Compute),
    ("$index", true, Name::Index),
    ("$deltatoken", false, Name::Deltatoken),
    ("$apply", true, Name::Apply),
    ("$levels", true, Name::Levels),
];

impl Name {
    /// The name with its `$`, as the error names it.
    fn label(self) -> &'static str {
        NAMES
            .iter()
            .find(|(_, _, name)| *name == self)
            .map_or("", |(word, _, _)| word)
    }

    fn not_implemented(self) -> Option<Feature> {
        match self {
            Name::Index => Some(Feature::Index),
            Name::Deltatoken => Some(Feature::DeltaToken),
            _ => None,
        }
    }
}

/// An option name of `among` and its `=`, consumed; `None` (cursor unmoved) otherwise.
///
/// Each name is one ABNF string (`"$filter" / "filter"`), so a `$` alone matches nothing. The
/// `$` may come percent-encoded (`%24filter`), as every client that builds its query with a
/// form encoder (Go's `url.Values`, Java's `URLEncoder`) writes it (T-3719): the ABNF reads that
/// as a custom option, which the surface refuses, so taking it as the system option changes no
/// URL that is served.
fn option_name(c: &mut Cursor<'_>, among: &[Name]) -> Option<Name> {
    let start = c.pos();
    let rest = c.rest();
    let encoded = rest.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("%24"));
    let (dollar, body) = match (rest.strip_prefix('$'), rest.get(3..)) {
        (Some(body), _) => (1, body),
        (None, Some(body)) if encoded => (3, body),
        _ => (0, rest),
    };
    for (word, plain_too, name) in NAMES {
        let word = word.trim_start_matches('$');
        let head = body.as_bytes().get(..word.len());
        if among.contains(&name)
            && (dollar > 0 || plain_too)
            && head.is_some_and(|h| h.eq_ignore_ascii_case(word.as_bytes()))
        {
            c.advance(word.len().saturating_add(dollar));
            if c.eq_sign() {
                return Some(name);
            }
            c.reset(start);
        }
    }
    None
}

const TOP_LEVEL: [Name; 16] = [
    Name::Filter,
    Name::Select,
    Name::Expand,
    Name::Orderby,
    Name::Top,
    Name::Skip,
    Name::Count,
    Name::Format,
    Name::Skiptoken,
    Name::Schemaversion,
    Name::Id,
    Name::Search,
    Name::Compute,
    Name::Index,
    Name::Deltatoken,
    Name::Apply,
];

/// The query parser: the cursor, the model, and the counters over the whole request.
pub(crate) struct Query<'x, 'a, 'l, 'm, M: Model> {
    pub(crate) c: &'x mut Cursor<'a>,
    pub(crate) model: &'m M,
    pub(crate) budget: Budget<'l>,
    pub(crate) expand_items: usize,
    pub(crate) select_items: usize,
}

impl<'m, M: Model> Query<'_, '_, '_, 'm, M> {
    /// The query string after `?`, up to the end of the URL.
    pub(crate) fn options(
        &mut self,
        scope: Scope<'m>,
        allowed: Allowed,
    ) -> Result<QueryOptions, UrlError> {
        let mut options = QueryOptions::default();
        let (mut applied, scope) = match allowed {
            Allowed::All => self.apply_first(scope)?,
            Allowed::Format | Allowed::EntityId => (None, scope),
        };
        let mut computed = match allowed {
            Allowed::All => self.compute_first(scope)?,
            Allowed::Format | Allowed::EntityId => None,
        };
        let mut seen: Vec<Name> = Vec::new();
        let mut n = 0;
        let max = self.budget.limits.max_query_options;
        loop {
            count(&mut n, max, LimitKind::QueryOptions, self.c)?;
            let start = self.c.pos();
            let among: &[Name] = match allowed {
                Allowed::All => &TOP_LEVEL,
                Allowed::Format => &[Name::Format],
                Allowed::EntityId => &[Name::Format, Name::Id],
            };
            if let Some(name) = option_name(self.c, among) {
                if seen.contains(&name) {
                    self.c.reset(start);
                    return Err(self.c.error(UrlErrorKind::DuplicateOption(name.label())));
                }
                seen.push(name);
                // parsed first; a second one is the duplicate refused above
                let first = match name {
                    Name::Apply => applied.take().map(|(end, steps)| {
                        options.apply = Some(steps);
                        end
                    }),
                    Name::Compute => computed.take().map(|(end, items)| {
                        options.compute = Some(items);
                        end
                    }),
                    _ => None,
                };
                match first {
                    Some(end) => self.c.reset(end),
                    None => self.system(scope, name, start, &mut options)?,
                }
            } else if allowed == Allowed::All && self.c.at() {
                self.alias(&mut options)?;
            } else {
                custom(self.c, &mut options)?;
            }
            if self.c.at_end() {
                break;
            }
            if !self.c.eat("&") {
                return Err(self.c.syntax("queryOption"));
            }
        }
        if allowed == Allowed::EntityId && options.id.is_none() {
            return Err(self.c.syntax("id"));
        }
        Ok(options)
    }

    /// The cursor after the `name=` of the first option `name`; unmoved and `false` without
    /// one.
    fn find(&mut self, name: Name) -> bool {
        let base = self.c.pos();
        // a raw `&` ends an option, but a string literal may hold one
        // (`pchar-no-SQUOTE`): one inside quotes starts nothing, so
        // `$filter=n eq 'a&$apply=...'` stays a filter (F-R2ANT4-016). An
        // escaped quote `''` toggles twice and keeps the literal open; `%27`
        // is a quote as well.
        let rest = self.c.rest().as_bytes();
        let mut quoted = false;
        let starts: Vec<usize> = std::iter::once(0)
            .chain((0..rest.len()).filter_map(|at| {
                let b = rest.get(at).copied();
                let quote = b == Some(b'\'')
                    || rest
                        .get(at..at.saturating_add(3))
                        .is_some_and(|t| t.eq_ignore_ascii_case(b"%27"));
                quoted ^= quote;
                (b == Some(b'&') && !quoted).then_some(at.saturating_add(1))
            }))
            .collect();
        for at in starts {
            self.c.reset(base.saturating_add(at));
            if option_name(self.c, &[name]).is_some() {
                return true;
            }
        }
        self.c.reset(base);
        false
    }

    /// `$apply` before every other option (Data Aggregation 4.0 section 3): the others name
    /// what it produces. Its transformations and where it ends, `None` without one, and the
    /// scope the other options resolve in.
    #[allow(clippy::type_complexity)]
    fn apply_first(
        &mut self,
        scope: Scope<'m>,
    ) -> Result<(Option<(usize, Vec<Transformation>)>, Scope<'m>), UrlError> {
        let base = self.c.pos();
        if !self.find(Name::Apply) {
            return Ok((None, scope));
        }
        let (steps, after) = self.apply(scope)?;
        let end = self.c.pos();
        self.c.reset(base);
        Ok((Some((end, steps)), after))
    }

    /// `$compute` before every other option but `$apply` (Protocol 11.2.1): its names resolve
    /// in the others wherever it stands. Its items and where it ends, `None` without one; the
    /// first one only, so a second is the duplicate the options loop answers.
    fn compute_first(
        &mut self,
        scope: Scope<'m>,
    ) -> Result<Option<(usize, Vec<Compute>)>, UrlError> {
        let base = self.c.pos();
        if !self.find(Name::Compute) {
            return Ok(None);
        }
        let items = self.compute(scope)?;
        let end = self.c.pos();
        self.c.reset(base);
        Ok(Some((end, items)))
    }

    /// `computeItem *( COMMA computeItem )`, `computeItem = commonExpr RWS "as" RWS
    /// computedProperty`; a name the resource has, or one given twice, is refused. Each
    /// expression counts against the request's expression budget.
    pub(crate) fn compute(&mut self, scope: Scope<'m>) -> Result<Vec<Compute>, UrlError> {
        let mut items: Vec<Compute> = Vec::new();
        loop {
            let expr = common_expr(self.c, self.model, scope, &mut self.budget)?;
            if !(self.c.rws() && self.c.eat_ci("as") && self.c.rws()) {
                return Err(self.c.syntax("computeItem"));
            }
            let at = self.c.pos();
            let Some(alias) = self.c.identifier()? else {
                return Err(self.c.syntax("computedProperty"));
            };
            if member(self.model, scope, alias).is_some()
                || items.iter().any(|i| i.alias == alias)
                || self.budget.computed.iter().any(|c| c == alias)
            {
                self.c.reset(at);
                return Err(self.c.syntax("computedProperty"));
            }
            items.push(Compute {
                expr,
                alias: alias.to_owned(),
            });
            if !self.c.comma() {
                // after the names an aggregating `$apply` produced
                self.budget
                    .computed
                    .extend(items.iter().map(|i| i.alias.clone()));
                return Ok(items);
            }
        }
    }

    fn system(
        &mut self,
        scope: Scope<'m>,
        name: Name,
        start: usize,
        options: &mut QueryOptions,
    ) -> Result<(), UrlError> {
        if let Some(feature) = name.not_implemented() {
            self.c.reset(start);
            return Err(self.c.not_implemented(feature));
        }
        match name {
            Name::Filter => {
                options.filter = Some(common_expr(self.c, self.model, scope, &mut self.budget)?);
            }
            Name::Orderby => options.orderby = Some(self.orderby(scope)?),
            Name::Select => options.select = Some(self.select(scope)?),
            Name::Expand => options.expand = Some(self.expand(scope, 1)?),
            Name::Top => options.top = Some(unsigned(self.c)?),
            Name::Skip => options.skip = Some(unsigned(self.c)?),
            Name::Count => options.count = Some(boolean(self.c)?),
            Name::Format => options.format = Some(format(self.c)?),
            Name::Skiptoken => options.skiptoken = Some(qchars(self.c, "skiptoken")?),
            Name::Search => {
                let limits = self.budget.limits;
                options.search = Some(search(self.c, limits, &mut self.budget.nodes, false)?);
            }
            Name::Id => options.id = Some(qchars(self.c, "IRI-in-query")?),
            _ => options.schemaversion = Some(schemaversion(self.c)?),
        }
        Ok(())
    }

    /// `aliasAndValue`: `@name=value`, the value a literal (an expression is not implemented).
    fn alias(&mut self, options: &mut QueryOptions) -> Result<(), UrlError> {
        let Some(name) = self.c.identifier()? else {
            return Err(self.c.syntax("parameterAlias"));
        };
        if !self.c.eq_sign() {
            return Err(self.c.syntax("EQ"));
        }
        let value_at = self.c.pos();
        if let Some(value) = literal(self.c, self.budget.limits)? {
            if end_of_option(self.c) {
                options.aliases.push((name.to_owned(), value));
                return Ok(());
            }
        }
        self.c.reset(value_at);
        Err(self.c.not_implemented(Feature::AliasExpression))
    }

    /// `$orderby`: `orderbyItem *( COMMA orderbyItem )`.
    pub(crate) fn orderby(&mut self, scope: Scope<'m>) -> Result<Vec<OrderBy>, UrlError> {
        let max = self.budget.limits.max_orderby_items;
        let mut items = Vec::new();
        loop {
            if items.len() >= max {
                return Err(self.c.limit(LimitKind::OrderbyItems, max));
            }
            let expr = common_expr(self.c, self.model, scope, &mut self.budget)?;
            let before = self.c.pos();
            let mut descending = false;
            if self.c.rws() && self.c.eat_ci("desc") {
                descending = true;
            } else {
                self.c.reset(before);
                if !(self.c.rws() && self.c.eat_ci("asc")) {
                    self.c.reset(before);
                }
            }
            items.push(OrderBy { expr, descending });
            if !self.c.comma() {
                return Ok(items);
            }
        }
    }

    /// `$select`: `selectItem *( COMMA selectItem )`.
    fn select(&mut self, scope: Scope<'m>) -> Result<Vec<SelectItem>, UrlError> {
        let mut items = Vec::new();
        loop {
            let max = self.budget.limits.max_select_items;
            count(&mut self.select_items, max, LimitKind::SelectItems, self.c)?;
            items.push(self.select_item(scope)?);
            if !self.c.comma() {
                return Ok(items);
            }
        }
    }

    fn select_item(&mut self, scope: Scope<'m>) -> Result<SelectItem, UrlError> {
        if self.c.star() {
            return Ok(SelectItem::Star);
        }
        let start = self.c.pos();
        if self.c.at() {
            let name = annotation(self.c, start)?;
            if self.c.open() {
                return Err(self.c.not_implemented(Feature::NestedOption));
            }
            return Ok(SelectItem::Annotation(name));
        }
        let mut path = Vec::new();
        let mut here = scope;
        loop {
            let at = self.c.pos();
            if self.c.at() {
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::AnnotationPath));
            }
            let Some(name) = self.c.identifier()? else {
                return Err(self.c.syntax("selectItem"));
            };
            if self.c.peek() == Some(b'.') {
                // A type cast, an operation, or `namespace.*`.
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::TypeCast));
            }
            let computed = path.is_empty() && self.budget.computed.iter().any(|c| c == name);
            let found = member(self.model, here, name)
                .or_else(|| computed.then_some(Member::Primitive { collection: false }));
            let Some(found) = found else {
                self.c.reset(at);
                return Err(self.c.error(UrlErrorKind::Unresolved));
            };
            path.push(name.to_owned());
            match found {
                Member::Complex { ty, .. } => {
                    if self.c.open() {
                        return Err(self.c.not_implemented(Feature::NestedOption));
                    }
                    if !self.c.slash() {
                        return Ok(SelectItem::Path(path));
                    }
                    here = Scope::Type(ty);
                }
                Member::Primitive { collection } => {
                    if collection && self.c.open() {
                        return Err(self.c.not_implemented(Feature::NestedOption));
                    }
                    return Ok(SelectItem::Path(path));
                }
                Member::Navigation { .. } => return Ok(SelectItem::Path(path)),
            }
        }
    }

    /// `$expand`: `expandItem *( COMMA expandItem )`, `depth` the level of these items.
    fn expand(&mut self, scope: Scope<'m>, depth: usize) -> Result<Vec<ExpandItem>, UrlError> {
        let max_depth = self.budget.limits.max_expand_depth;
        if depth > max_depth {
            return Err(self.c.limit(LimitKind::ExpandDepth, max_depth));
        }
        let mut items = Vec::new();
        loop {
            let max = self.budget.limits.max_expand_items;
            count(&mut self.expand_items, max, LimitKind::ExpandItems, self.c)?;
            items.push(self.expand_item(scope, depth)?);
            if !self.c.comma() {
                return Ok(items);
            }
        }
    }

    fn expand_item(&mut self, scope: Scope<'m>, depth: usize) -> Result<ExpandItem, UrlError> {
        let start = self.c.pos();
        if self.c.eat("$value") {
            self.c.reset(start);
            return Err(self.c.not_implemented(Feature::MediaValue));
        }
        if self.c.star() {
            let mut levels = None;
            let kind = if self.c.eat("/$ref") {
                ExpandKind::Ref
            } else {
                if self.c.open() {
                    if option_name(self.c, &[Name::Levels]).is_none() {
                        return Err(self.c.syntax("levels"));
                    }
                    levels = Some(self.levels(depth)?);
                    if !self.c.close() {
                        return Err(self.c.syntax("CLOSE"));
                    }
                }
                ExpandKind::Entities {
                    select: None,
                    expand: None,
                    levels,
                }
            };
            return Ok(ExpandItem {
                path: ExpandPath::Star,
                kind,
            });
        }
        if self.c.at() {
            self.c.reset(start);
            return Err(self.c.not_implemented(Feature::AnnotationPath));
        }
        let mut path = Vec::new();
        let mut here = scope;
        loop {
            let at = self.c.pos();
            if !path.is_empty() && self.c.star() {
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::PathStar));
            }
            let Some(name) = self.c.identifier()? else {
                return Err(self.c.syntax("expandPath"));
            };
            if self.c.peek() == Some(b'.') {
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::TypeCast));
            }
            let found = member(self.model, here, name);
            path.push(name.to_owned());
            match found {
                Some(Member::Complex { ty, .. }) => {
                    if !self.c.slash() {
                        return Err(self.c.syntax("expandPath"));
                    }
                    here = Scope::Type(ty);
                }
                Some(Member::Navigation { ty, .. }) => {
                    let kind = self.expand_kind(Scope::Type(ty), depth)?;
                    return Ok(ExpandItem {
                        path: ExpandPath::Path(path),
                        kind,
                    });
                }
                Some(Member::Primitive { .. }) | None => {
                    self.c.reset(at);
                    return Err(self.c.error(UrlErrorKind::Unresolved));
                }
            }
        }
    }

    /// What follows a navigation property in `$expand`.
    fn expand_kind(&mut self, target: Scope<'m>, depth: usize) -> Result<ExpandKind, UrlError> {
        let start = self.c.pos();
        if self.c.slash() {
            let at = self.c.pos();
            if self.c.identifier()?.is_some() {
                // A type cast, qualified or by the default namespace (4.01).
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::TypeCast));
            }
            self.c.reset(start);
        }
        if self.c.eat("/$ref") {
            if self.c.open() {
                let refused = [
                    Name::Filter,
                    Name::Search,
                    Name::Orderby,
                    Name::Skip,
                    Name::Top,
                    Name::Count,
                ];
                return Err(self.nested_refused(&refused));
            }
            return Ok(ExpandKind::Ref);
        }
        if self.c.eat("/$count") {
            self.c.reset(start);
            return Err(self.c.not_implemented(Feature::NestedCount));
        }
        let (mut select, mut expand, mut levels) = (None, None, None);
        if self.c.open() {
            // the `$compute` names are the resource's, not the expanded entities'
            let computed = std::mem::take(&mut self.budget.computed);
            let nested = self.expand_options(target, depth, &mut select, &mut expand, &mut levels);
            self.budget.computed = computed;
            nested?;
        }
        Ok(ExpandKind::Entities {
            select,
            expand,
            levels,
        })
    }

    /// The options of an expanded navigation after its `OPEN`, up to its `CLOSE`.
    fn expand_options(
        &mut self,
        target: Scope<'m>,
        depth: usize,
        select: &mut Option<Vec<SelectItem>>,
        expand: &mut Option<Vec<ExpandItem>>,
        levels: &mut Option<Levels>,
    ) -> Result<(), UrlError> {
        loop {
            match option_name(self.c, &[Name::Select, Name::Expand, Name::Levels]) {
                Some(Name::Select) => *select = Some(self.select(target)?),
                Some(Name::Expand) => {
                    *expand = Some(self.expand(target, depth.saturating_add(1))?);
                }
                Some(_) => *levels = Some(self.levels(depth)?),
                None => {
                    let refused = [
                        Name::Filter,
                        Name::Search,
                        Name::Orderby,
                        Name::Skip,
                        Name::Top,
                        Name::Count,
                        Name::Compute,
                    ];
                    return Err(self.nested_refused(&refused));
                }
            }
            if self.c.close() {
                break;
            }
            if !self.c.semi() {
                return Err(self.c.syntax("SEMI"));
            }
        }
        Ok(())
    }

    /// A nested option that is not implemented when it is one of `refused` or an alias, and a
    /// syntax error otherwise.
    fn nested_refused(&mut self, refused: &[Name]) -> UrlError {
        let start = self.c.pos();
        if option_name(self.c, refused).is_some() || self.c.at() {
            self.c.reset(start);
            return self.c.not_implemented(Feature::NestedOption);
        }
        self.c.syntax("expandOption")
    }

    /// The value of `$levels`: `oneToNine *DIGIT / "max"`, within the expand depth left below
    /// `depth`.
    fn levels(&mut self, depth: usize) -> Result<Levels, UrlError> {
        if self.c.eat_ci("max") {
            return Ok(Levels::Max);
        }
        let start = self.c.pos();
        if self.c.eat_if(|b| (b'1'..=b'9').contains(&b)).is_none() {
            return Err(self.c.syntax("levels"));
        }
        self.c.digits();
        let max_depth = self.budget.limits.max_expand_depth;
        let room = max_depth.saturating_sub(depth.saturating_sub(1));
        let levels = self.c.slice(start).parse::<u64>().ok();
        match levels {
            Some(n) if usize::try_from(n).is_ok_and(|n| n <= room) => Ok(Levels::Count(n)),
            _ => {
                self.c.reset(start);
                Err(self.c.limit(LimitKind::ExpandDepth, max_depth))
            }
        }
    }
}

/// The member `name` of `scope`.
pub(crate) fn member<'m, M: Model>(
    model: &'m M,
    scope: Scope<'_>,
    name: &str,
) -> Option<Member<'m>> {
    match scope {
        Scope::Type(ty) => model.member(ty, name),
        Scope::Primitive => None,
    }
}

/// `1*DIGIT` as a `u64`.
pub(crate) fn unsigned(c: &mut Cursor<'_>) -> Result<u64, UrlError> {
    let start = c.pos();
    let digits = c.digits();
    if digits.is_empty() {
        return Err(c.syntax("DIGIT"));
    }
    digits.parse().map_err(|_| {
        c.reset(start);
        c.limit(LimitKind::Number, usize::MAX)
    })
}

/// `boolean`: `true` or `false`, any case.
fn boolean(c: &mut Cursor<'_>) -> Result<bool, UrlError> {
    if c.eat_ci("true") {
        Ok(true)
    } else if c.eat_ci("false") {
        Ok(false)
    } else {
        Err(c.syntax("boolean"))
    }
}

fn end_of_option(c: &Cursor<'_>) -> bool {
    c.at_end() || c.peek() == Some(b'&')
}

/// `pchar` without its percent-encoded part.
fn is_pchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~$'=!()*+,;:@".contains(&b)
}

/// `1*pchar` up to `stop`, its length.
fn pchars(c: &mut Cursor<'_>, stop: u8) -> usize {
    let start = c.pos();
    while c.peek() != Some(stop) && (pct_encoded(c).is_some() || c.eat_if(is_pchar).is_some()) {}
    c.pos().saturating_sub(start)
}

/// `$format`: `atom`, `json`, `xml`, or `1*pchar "/" 1*pchar`.
fn format(c: &mut Cursor<'_>) -> Result<Format, UrlError> {
    let start = c.pos();
    for (word, format) in [
        ("atom", Format::Atom),
        ("json", Format::Json),
        ("xml", Format::Xml),
    ] {
        if c.eat_ci(word) && end_of_option(c) {
            return Ok(format);
        }
        c.reset(start);
    }
    if pchars(c, b'/') > 0 && c.eat("/") && pchars(c, b'&') > 0 {
        return Ok(Format::MediaType(c.slice(start).to_owned()));
    }
    // A form encoder writes the separator as `%2F` (T-3719, T-3742): the value as written,
    // with its decoded form `type/subtype`.
    c.reset(start);
    if pchars(c, b'&') > 0 {
        let written = c.slice(start);
        let decoded = crate::percent_decoded(written).unwrap_or_default();
        if decoded
            .split_once('/')
            .is_some_and(|(t, s)| !t.is_empty() && !s.is_empty())
        {
            return Ok(Format::MediaType(written.to_owned()));
        }
    }
    Err(c.syntax("format"))
}

/// `qchar-no-AMP` without its percent-encoded part.
fn is_qchar_no_amp(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!()*+,;:@/?$'=".contains(&b)
}

/// `*qchar-no-AMP` that `accept` takes.
fn qchars_while(c: &mut Cursor<'_>, accept: impl Fn(u8) -> bool) {
    while pct_encoded(c).is_some() || c.eat_if(&accept).is_some() {}
}

/// `1*qchar-no-AMP`, as written.
fn qchars(c: &mut Cursor<'_>, rule: &'static str) -> Result<String, UrlError> {
    let start = c.pos();
    qchars_while(c, is_qchar_no_amp);
    if c.pos() == start {
        return Err(c.syntax(rule));
    }
    Ok(c.slice(start).to_owned())
}

/// `$schemaversion`: `*` or `1*unreserved`.
fn schemaversion(c: &mut Cursor<'_>) -> Result<String, UrlError> {
    if c.star() {
        return Ok("*".to_owned());
    }
    let start = c.pos();
    while c
        .eat_if(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        .is_some()
    {}
    if c.pos() == start {
        return Err(c.syntax("schemaversion"));
    }
    Ok(c.slice(start).to_owned())
}

/// `customQueryOption`: a name not starting with `$` or `@`, and an optional value, as written.
fn custom(c: &mut Cursor<'_>, options: &mut QueryOptions) -> Result<(), UrlError> {
    let start = c.pos();
    let first = |b: u8| is_qchar_no_amp(b) && !b"=@$".contains(&b);
    if pct_encoded(c).is_none() && c.eat_if(first).is_none() {
        return Err(c.syntax("queryOption"));
    }
    qchars_while(c, |b| is_qchar_no_amp(b) && b != b'=');
    let name = c.slice(start).to_owned();
    let value = if c.eat("=") {
        let value_start = c.pos();
        qchars_while(c, is_qchar_no_amp);
        Some(c.slice(value_start).to_owned())
    } else {
        None
    };
    options.custom.push((name, value));
    Ok(())
}
