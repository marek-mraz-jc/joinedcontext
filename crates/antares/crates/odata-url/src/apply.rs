// SPDX-License-Identifier: EUPL-1.2
//! `$apply` (OData Data Aggregation 4.0 section 3): `identity`, `filter`, `search`, `orderby`,
//! `top`, `skip`, `compute`, `aggregate`, `groupby` (T-3937), the six of `topcount` to
//! `bottompercent` and `concat` (T-3985). The hierarchy and join transformations, `expand`,
//! `nest`, rollup, a transformation of the groups other than `aggregate`, custom aggregates and
//! custom methods are not implemented.

use crate::cursor::count;
use crate::expr::{common_expr, Scope};
use crate::model::{Member, Model};
use crate::query::{member, unsigned, Query};
use crate::search::search;
use crate::{
    Aggregate, AggregateWith, Feature, LimitKind, PartitionBy, Transformation, UrlError,
    UrlErrorKind,
};

/// Data Aggregation 4.0 transformations the parser does not serve, and `rollup`.
const OTHER: [&str; 9] = [
    "expand",
    "nest",
    "addnested",
    "join",
    "outerjoin",
    "ancestors",
    "descendants",
    "traverse",
    "rollup",
];

/// The transformations that keep the instances with the highest or lowest values.
const PARTITIONS: [(&str, PartitionBy, bool); 6] = [
    ("topcount", PartitionBy::Count, true),
    ("topsum", PartitionBy::Sum, true),
    ("toppercent", PartitionBy::Percent, true),
    ("bottomcount", PartitionBy::Count, false),
    ("bottomsum", PartitionBy::Sum, false),
    ("bottompercent", PartitionBy::Percent, false),
];

const METHODS: [(&str, AggregateWith); 5] = [
    ("sum", AggregateWith::Sum),
    ("min", AggregateWith::Min),
    ("max", AggregateWith::Max),
    ("average", AggregateWith::Average),
    ("countdistinct", AggregateWith::CountDistinct),
];

impl<'m, M: Model> Query<'_, '_, '_, 'm, M> {
    /// `applyExpr`: transformations separated by `/`, and the scope after them. After one that
    /// aggregates, the scope holds no member and `budget.computed` names what it produced.
    ///
    /// Every transformation costs one expression node, `identity` and each `concat` included, and
    /// a sequence inside a `concat` is one level of `max_expr_depth`: neither a long chain nor a
    /// deep nesting parses for free, and the nesting is refused before it can exhaust the stack
    /// (T-4017).
    pub(crate) fn apply(
        &mut self,
        scope: Scope<'m>,
    ) -> Result<(Vec<Transformation>, Scope<'m>), UrlError> {
        count(
            &mut self.budget.apply_depth,
            self.budget.limits.max_expr_depth,
            LimitKind::ExprDepth,
            self.c,
        )?;
        let parsed = self.sequence(scope);
        self.budget.apply_depth = self.budget.apply_depth.saturating_sub(1);
        parsed
    }

    fn sequence(&mut self, scope: Scope<'m>) -> Result<(Vec<Transformation>, Scope<'m>), UrlError> {
        let mut steps = Vec::new();
        let mut here = scope;
        loop {
            count(
                &mut self.budget.nodes,
                self.budget.limits.max_expr_nodes,
                LimitKind::ExprNodes,
                self.c,
            )?;
            steps.push(self.transformation(&mut here)?);
            if !self.c.slash() {
                return Ok((steps, here));
            }
        }
    }

    fn transformation(&mut self, here: &mut Scope<'m>) -> Result<Transformation, UrlError> {
        let start = self.c.pos();
        let Some(word) = self.c.identifier()? else {
            return Err(self.c.syntax("applyTrafo"));
        };
        let word = word.to_ascii_lowercase();
        if word == "identity" {
            return Ok(Transformation::Identity);
        }
        if self.c.peek() == Some(b'.') || OTHER.contains(&word.as_str()) {
            // a custom transformation (a function), or one not served
            self.c.reset(start);
            return Err(self.c.not_implemented(Feature::Apply));
        }
        if !self.c.open() {
            return Err(self.c.syntax("applyTrafo"));
        }
        self.c.bws();
        let step = match word.as_str() {
            "filter" => {
                Transformation::Filter(common_expr(self.c, self.model, *here, &mut self.budget)?)
            }
            "orderby" => Transformation::OrderBy(self.orderby(*here)?),
            "top" => Transformation::Top(unsigned(self.c)?),
            "skip" => Transformation::Skip(unsigned(self.c)?),
            "aggregate" => {
                let items = self.aggregates(*here, &[])?;
                self.produced(here, &[], &items);
                Transformation::Aggregate(items)
            }
            "groupby" => self.groupby(here)?,
            "compute" => Transformation::Compute(self.compute(*here)?),
            "search" => {
                let limits = self.budget.limits;
                Transformation::Search(search(self.c, limits, &mut self.budget.nodes, true)?)
            }
            "concat" => self.concat(here, start)?,
            partition => {
                let Some(&(_, by, top)) = PARTITIONS.iter().find(|(name, ..)| *name == partition)
                else {
                    self.c.reset(start);
                    return Err(self.c.syntax("applyTrafo"));
                };
                self.partition(*here, by, top)?
            }
        };
        self.c.bws();
        if !self.c.close() {
            return Err(self.c.syntax("CLOSE"));
        }
        Ok(step)
    }

    /// `topcount` to `bottompercent` after its `OPEN`: `n, expr`.
    fn partition(
        &mut self,
        here: Scope<'m>,
        by: PartitionBy,
        top: bool,
    ) -> Result<Transformation, UrlError> {
        let n = common_expr(self.c, self.model, here, &mut self.budget)?;
        self.c.bws();
        if !self.c.comma() {
            return Err(self.c.syntax("COMMA"));
        }
        self.c.bws();
        let expr = common_expr(self.c, self.model, here, &mut self.budget)?;
        Ok(Transformation::Partition { by, top, n, expr })
    }

    /// `concat` after its `OPEN`: two or more sequences over the same input. Their results
    /// follow one another, so they all keep the instances or all aggregate (the other mix is
    /// not implemented); after sequences that aggregate, the names any of them produced resolve.
    fn concat(&mut self, here: &mut Scope<'m>, start: usize) -> Result<Transformation, UrlError> {
        let before = self.budget.computed.clone();
        let mut branches: Vec<Vec<Transformation>> = Vec::new();
        let mut produced: Vec<String> = Vec::new();
        loop {
            self.budget.computed.clone_from(&before);
            let (steps, _) = self.apply(*here)?;
            if steps.iter().any(Transformation::aggregates) {
                for name in &self.budget.computed {
                    if !produced.contains(name) {
                        produced.push(name.clone());
                    }
                }
            }
            branches.push(steps);
            self.c.bws();
            if !self.c.comma() {
                break;
            }
            self.c.bws();
        }
        if branches.len() < 2 {
            return Err(self.c.syntax("COMMA"));
        }
        let aggregating = branches
            .iter()
            .filter(|b| b.iter().any(Transformation::aggregates));
        match aggregating.count() {
            0 => self.budget.computed = before,
            n if n == branches.len() => {
                *here = Scope::Primitive;
                self.budget.computed = produced;
            }
            _ => {
                self.c.reset(start);
                return Err(self.c.not_implemented(Feature::Apply));
            }
        }
        Ok(Transformation::Concat(branches))
    }

    /// After an aggregation only `properties` and the aliases of `items` resolve.
    fn produced(&mut self, here: &mut Scope<'m>, properties: &[String], items: &[Aggregate]) {
        *here = Scope::Primitive;
        self.budget.computed = properties
            .iter()
            .cloned()
            .chain(items.iter().map(|i| i.alias().to_owned()))
            .collect();
    }

    /// `groupby` after its `OPEN`: `(groupingProperty, ...)`, then `aggregate(...)` or nothing.
    fn groupby(&mut self, here: &mut Scope<'m>) -> Result<Transformation, UrlError> {
        if !self.c.open() {
            return Err(self.c.syntax("groupbyList"));
        }
        let mut properties: Vec<String> = Vec::new();
        loop {
            self.c.bws();
            let at = self.c.pos();
            let Some(name) = self.c.identifier()? else {
                return Err(self.c.syntax("groupingProperty"));
            };
            if self.c.slash() || self.c.open() || self.c.peek() == Some(b'.') {
                // a path, `rollup(...)`, or a type cast
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::Apply));
            }
            let computed = self.budget.computed.iter().any(|c| c == name);
            let found = member(self.model, *here, name)
                .or_else(|| computed.then_some(Member::Primitive { collection: false }));
            match found {
                Some(Member::Primitive { collection: false }) => {}
                Some(_) => {
                    self.c.reset(at);
                    return Err(self.c.not_implemented(Feature::Apply));
                }
                None => {
                    self.c.reset(at);
                    return Err(self.c.error(UrlErrorKind::Unresolved));
                }
            }
            if properties.iter().any(|p| p == name) {
                self.c.reset(at);
                return Err(self.c.syntax("groupingProperty"));
            }
            properties.push(name.to_owned());
            self.c.bws();
            if !self.c.comma() {
                break;
            }
        }
        if !self.c.close() {
            return Err(self.c.syntax("CLOSE"));
        }
        self.c.bws();
        let mut aggregate = None;
        if self.c.comma() {
            self.c.bws();
            let at = self.c.pos();
            let word = self.c.identifier()?;
            if !word.is_some_and(|w| w.eq_ignore_ascii_case("aggregate")) {
                // any other transformation of the groups
                self.c.reset(at);
                return Err(self.c.not_implemented(Feature::Apply));
            }
            if !self.c.open() {
                return Err(self.c.syntax("OPEN"));
            }
            self.c.bws();
            let items = self.aggregates(*here, &properties)?;
            self.c.bws();
            if !self.c.close() {
                return Err(self.c.syntax("CLOSE"));
            }
            aggregate = Some(items);
        }
        self.produced(here, &properties, aggregate.as_deref().unwrap_or_default());
        Ok(Transformation::GroupBy {
            properties,
            aggregate,
        })
    }

    /// `aggregateExpr *( COMMA aggregateExpr )`; an alias given twice, or one of `taken`, is
    /// refused.
    fn aggregates(
        &mut self,
        scope: Scope<'m>,
        taken: &[String],
    ) -> Result<Vec<Aggregate>, UrlError> {
        let mut items: Vec<Aggregate> = Vec::new();
        loop {
            let item = if self.c.eat("$count") {
                self.aggregate_alias(|alias| Aggregate::Count { alias })?
            } else {
                let expr = common_expr(self.c, self.model, scope, &mut self.budget)?;
                let at = self.c.pos();
                if !(self.c.rws() && self.c.eat_ci("with") && self.c.rws()) {
                    self.c.reset(at);
                    // a custom aggregate (`aggregate(Forecast)`) or `from`
                    return Err(self.c.not_implemented(Feature::Apply));
                }
                let with = self.method()?;
                self.aggregate_alias(|alias| Aggregate::With { expr, with, alias })?
            };
            let alias = item.alias();
            if taken.iter().any(|t| t == alias) || items.iter().any(|i| i.alias() == alias) {
                return Err(self.c.syntax("alias"));
            }
            items.push(item);
            self.c.bws();
            if !self.c.comma() {
                return Ok(items);
            }
            self.c.bws();
        }
    }

    /// An aggregation method: a standard one, a custom (qualified) one is not implemented.
    fn method(&mut self) -> Result<AggregateWith, UrlError> {
        let at = self.c.pos();
        let word = self.c.identifier()?.unwrap_or_default();
        if self.c.peek() == Some(b'.') {
            self.c.reset(at);
            return Err(self.c.not_implemented(Feature::Apply));
        }
        if let Some((_, with)) = METHODS.iter().find(|(n, _)| n.eq_ignore_ascii_case(word)) {
            return Ok(*with);
        }
        self.c.reset(at);
        Err(self.c.syntax("aggregateMethod"))
    }

    /// `RWS "as" RWS alias`, made into an item by `item`.
    fn aggregate_alias(
        &mut self,
        item: impl FnOnce(String) -> Aggregate,
    ) -> Result<Aggregate, UrlError> {
        if !(self.c.rws() && self.c.eat_ci("as") && self.c.rws()) {
            return Err(self.c.syntax("aggregateExpr"));
        }
        match self.c.identifier()? {
            Some(alias) => Ok(item(alias.to_owned())),
            None => Err(self.c.syntax("alias")),
        }
    }
}
