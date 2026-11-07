// SPDX-License-Identifier: EUPL-1.2
//! `resourcePath` (ABNF section 1) for the resource paths of T-3441 Q29 to Q35.

use crate::cursor::{count, Cursor};
use crate::expr::Scope;
use crate::literal::literal;
use crate::model::{Member, Model};
use crate::{Feature, Key, KeyValue, LimitKind, Limits, Segment, UrlError, UrlErrorKind};

/// What the path addresses so far, which decides what may follow.
#[derive(Debug, Clone, Copy)]
enum At<'m> {
    /// A collection of entities of a type.
    Entities(&'m str),
    /// One entity of a type.
    Entity(&'m str),
    /// One complex value of a type.
    Complex(&'m str),
    /// A collection of complex values of a type.
    Complexes(&'m str),
    /// One primitive value.
    Primitive,
    /// A collection of primitives.
    Primitives,
    /// After `$count`, `$ref`, `$value` or `$query`: nothing may follow; the scope is kept.
    End(Scope<'m>),
}

impl<'m> At<'m> {
    /// The scope query options of this resource resolve names in.
    fn scope(self) -> Scope<'m> {
        match self {
            At::Entities(ty) | At::Entity(ty) | At::Complex(ty) | At::Complexes(ty) => {
                Scope::Type(ty)
            }
            At::Primitive | At::Primitives => Scope::Primitive,
            At::End(scope) => scope,
        }
    }
}

struct PathParser<'x, 'a, 'm, M: Model> {
    c: &'x mut Cursor<'a>,
    model: &'m M,
    limits: &'x Limits,
    segments: Vec<Segment>,
}

/// `resourcePath` of the subset, and the scope its query options resolve names in.
pub(crate) fn resource_path<'m, M: Model>(
    c: &mut Cursor<'_>,
    model: &'m M,
    limits: &Limits,
) -> Result<(Vec<Segment>, Scope<'m>), UrlError> {
    let mut p = PathParser {
        c,
        model,
        limits,
        segments: Vec::new(),
    };
    let start = p.c.pos();
    let Some(name) = p.c.identifier()? else {
        return Err(p.c.syntax("resourcePath"));
    };
    let Some(ty) = model.entity_set(name) else {
        if p.c.peek() == Some(b'.') {
            return Err(p.c.syntax("resourcePath"));
        }
        p.c.reset(start);
        return Err(p.c.error(UrlErrorKind::Unresolved));
    };
    p.push(Segment::EntitySet(name.to_owned()))?;
    let mut at = At::Entities(ty);
    loop {
        let before = p.c.pos();
        let Some(next) = p.step(at)? else {
            p.c.reset(before);
            return Ok((p.segments, at.scope()));
        };
        at = next;
    }
}

impl<'m, M: Model> PathParser<'_, '_, 'm, M> {
    fn push(&mut self, segment: Segment) -> Result<(), UrlError> {
        let mut n = self.segments.len();
        count(
            &mut n,
            self.limits.max_path_segments,
            LimitKind::PathSegments,
            self.c,
        )?;
        self.segments.push(segment);
        Ok(())
    }

    /// One more segment after `at`, `None` when nothing that may follow is here.
    fn step(&mut self, at: At<'m>) -> Result<Option<At<'m>>, UrlError> {
        if let At::End(_) = at {
            return Ok(None);
        }
        if let At::Entities(ty) = at {
            if self.c.open() {
                let key = self.key()?;
                self.push(Segment::Key(key))?;
                return Ok(Some(At::Entity(ty)));
            }
        }
        if !self.c.eat("/") {
            return Ok(None);
        }
        // `resourcePath` has no empty segment: `Buildings/` is malformed, not
        // `Buildings` nor a name that fails to resolve (T-3731).
        if matches!(self.c.peek(), None | Some(b'/' | b'?')) {
            return Err(self.c.syntax("path segment"));
        }
        let start = self.c.pos();
        let dollar = [
            ("$count", Segment::Count),
            ("$ref", Segment::Ref),
            ("$value", Segment::Value),
            ("$query", Segment::Query),
        ];
        for (word, segment) in dollar {
            // Only a segment that may follow here is matched at all.
            let allowed = match segment {
                Segment::Count => matches!(at, At::Entities(_) | At::Complexes(_) | At::Primitives),
                Segment::Ref => matches!(at, At::Entities(_) | At::Entity(_)),
                Segment::Value => matches!(at, At::Primitive | At::Entity(_)),
                _ => true,
            };
            if allowed && self.c.eat(word) {
                if matches!((&segment, at), (Segment::Value, At::Entity(_))) {
                    self.c.reset(start);
                    return Err(self.c.not_implemented(Feature::MediaValue));
                }
                self.push(segment)?;
                return Ok(Some(At::End(at.scope())));
            }
        }
        for (word, feature) in [
            ("$filter", Feature::FilterSegment),
            ("$each", Feature::Each),
        ] {
            if self.c.eat(word) {
                self.c.reset(start);
                return Err(self.c.not_implemented(feature));
            }
        }
        if matches!(at, At::Complexes(_) | At::Primitives) {
            self.c.eat("-");
            if !self.c.digits().is_empty() {
                self.c.reset(start);
                return Err(self.c.not_implemented(Feature::Ordinal));
            }
            self.c.reset(start);
        }
        self.member(at, start)
    }

    /// A member segment after `at`, its name at `start`.
    fn member(&mut self, at: At<'m>, start: usize) -> Result<Option<At<'m>>, UrlError> {
        let Some(name) = self.c.identifier()? else {
            if let At::Entities(_) = at {
                // Key-as-segment with a key that is no identifier (`Orders/1`).
                self.c.reset(start);
                return Err(self.c.error(UrlErrorKind::Unresolved));
            }
            return Ok(None);
        };
        if self.c.peek() == Some(b'.') {
            // A type cast or a bound operation.
            self.c.reset(start);
            return Err(self.c.not_implemented(Feature::TypeCast));
        }
        let ty = match at {
            At::Entity(ty) | At::Complex(ty) => ty,
            At::Entities(_) => {
                // Key-as-segment (URL Conventions 4.3.6) is not offered: the segment does not
                // resolve (T-3441 Q38).
                self.c.reset(start);
                return Err(self.c.error(UrlErrorKind::Unresolved));
            }
            _ => return Ok(None),
        };
        let Some(member) = self.model.member(ty, name) else {
            self.c.reset(start);
            return Err(self.c.error(UrlErrorKind::Unresolved));
        };
        let (segment, next) = match member {
            Member::Navigation {
                ty,
                collection: true,
            } => (Segment::Navigation(name.to_owned()), At::Entities(ty)),
            Member::Navigation {
                ty,
                collection: false,
            } => (Segment::Navigation(name.to_owned()), At::Entity(ty)),
            Member::Complex {
                ty,
                collection: true,
            } => (Segment::Property(name.to_owned()), At::Complexes(ty)),
            Member::Complex {
                ty,
                collection: false,
            } => (Segment::Property(name.to_owned()), At::Complex(ty)),
            Member::Primitive { collection: true } => {
                (Segment::Property(name.to_owned()), At::Primitives)
            }
            Member::Primitive { collection: false } => {
                (Segment::Property(name.to_owned()), At::Primitive)
            }
        };
        self.push(segment)?;
        Ok(Some(next))
    }

    /// `keyPredicate` after its `OPEN`: a single value or `name=value` pairs, then `CLOSE`.
    fn key(&mut self) -> Result<Key, UrlError> {
        let start = self.c.pos();
        if let Some(value) = self.key_value()? {
            if self.c.close() {
                return Ok(Key::Single(value));
            }
        }
        self.c.reset(start);
        let mut pairs = Vec::new();
        loop {
            let Some(name) = self.c.identifier()? else {
                return Err(self.c.syntax("keyPredicate"));
            };
            let name = name.to_owned();
            if !self.c.eat("=") {
                return Err(self.c.syntax("EQ"));
            }
            let Some(value) = self.key_value()? else {
                return Err(self.c.syntax("keyPropertyValue"));
            };
            pairs.push((name, value));
            if self.c.close() {
                return Ok(Key::Compound(pairs));
            }
            if !self.c.comma() {
                return Err(self.c.syntax("CLOSE"));
            }
        }
    }

    /// `parameterAlias / keyPropertyValue`.
    fn key_value(&mut self) -> Result<Option<KeyValue>, UrlError> {
        let start = self.c.pos();
        if self.c.at() {
            if let Some(alias) = self.c.identifier()? {
                return Ok(Some(KeyValue::Alias(alias.to_owned())));
            }
            self.c.reset(start);
            return Ok(None);
        }
        Ok(literal(self.c, self.limits)?.map(KeyValue::Literal))
    }
}
