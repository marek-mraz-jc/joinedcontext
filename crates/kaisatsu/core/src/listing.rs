//! What the paged list calls of [`Store`] take and answer: field matches and one page (STO-18).

use crate::error::Result;
use crate::model::{GroupRepresentation, UserRepresentation};
use crate::store::Store;

/// The most rows one list call answers, whatever its caller asks for (STO-10).
pub const MAX_PAGE: usize = 1000;

/// How a field is compared. ASCII case is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchOp {
    Equals,
    Contains,
    StartsWith,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonField {
    Id,
    Username,
    Email,
    FirstName,
    LastName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupField {
    Id,
    Name,
}

/// One condition of a filter; a list of them is a conjunction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMatch<F> {
    pub field: F,
    pub op: MatchOp,
    pub value: String,
}

impl<F> FieldMatch<F> {
    fn holds(&self, text: &str) -> bool {
        let (text, wanted) = (text.to_ascii_lowercase(), self.value.to_ascii_lowercase());
        match self.op {
            MatchOp::Equals => text == wanted,
            MatchOp::Contains => text.contains(&wanted),
            MatchOp::StartsWith => text.starts_with(&wanted),
        }
    }
}

impl FieldMatch<PersonField> {
    pub fn matches(&self, person: &UserRepresentation) -> bool {
        self.holds(match self.field {
            PersonField::Id => &person.id,
            PersonField::Username => &person.username,
            PersonField::Email => person.email.as_deref().unwrap_or_default(),
            PersonField::FirstName => person.first_name.as_deref().unwrap_or_default(),
            PersonField::LastName => person.last_name.as_deref().unwrap_or_default(),
        })
    }
}

impl FieldMatch<GroupField> {
    pub fn matches(&self, group: &GroupRepresentation) -> bool {
        self.holds(match self.field {
            GroupField::Id => &group.id,
            GroupField::Name => &group.name,
        })
    }
}

/// One page and the number of rows its filter selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage<T> {
    pub items: Vec<T>,
    pub total: usize,
}

impl<T> Default for ListPage<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
        }
    }
}

/// A group with the first [`MAX_PAGE`] of its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupWithMembers {
    pub group: GroupRepresentation,
    pub members: Vec<UserRepresentation>,
}

/// Every person of the realm, read page by page: for export and the realm split, never a route.
pub async fn all_users(store: &dyn Store, realm_id: &str) -> Result<Vec<UserRepresentation>> {
    let mut all = Vec::new();
    loop {
        let page = store
            .list_users(realm_id, None, None, None, None, false, all.len(), MAX_PAGE)
            .await?;
        let full = page.len() == MAX_PAGE;
        all.extend(page);
        if !full {
            return Ok(all);
        }
    }
}

/// Every member of the group, read page by page: for the realm split and the export, never a route.
pub async fn all_group_members(
    store: &dyn Store,
    realm_id: &str,
    group_id: &str,
) -> Result<Vec<UserRepresentation>> {
    let mut all = Vec::new();
    loop {
        let page = store
            .list_group_members(realm_id, None, group_id, all.len(), MAX_PAGE)
            .await?;
        let full = page.len() == MAX_PAGE;
        all.extend(page);
        if !full {
            return Ok(all);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m<F>(field: F, op: MatchOp, value: &str) -> FieldMatch<F> {
        FieldMatch {
            field,
            op,
            value: value.into(),
        }
    }

    #[test]
    fn a_person_matches_by_each_field_ignoring_case() {
        let ada = UserRepresentation {
            id: "U-1".into(),
            username: "Ada".into(),
            email: Some("Ada@Example.org".into()),
            first_name: Some("Augusta".into()),
            last_name: None,
            ..UserRepresentation::default()
        };
        assert!(m(PersonField::Id, MatchOp::Equals, "u-1").matches(&ada));
        assert!(!m(PersonField::Id, MatchOp::Equals, "u-").matches(&ada));
        assert!(m(PersonField::Username, MatchOp::StartsWith, "AD").matches(&ada));
        assert!(!m(PersonField::Username, MatchOp::StartsWith, "da").matches(&ada));
        assert!(m(PersonField::Email, MatchOp::Contains, "EXAMPLE").matches(&ada));
        assert!(!m(PersonField::Email, MatchOp::Contains, "test").matches(&ada));
        assert!(m(PersonField::FirstName, MatchOp::Equals, "augusta").matches(&ada));
        assert!(!m(PersonField::FirstName, MatchOp::Equals, "ada").matches(&ada));
        assert!(
            m(PersonField::LastName, MatchOp::Equals, "").matches(&ada),
            "absent is empty"
        );
        assert!(!m(PersonField::LastName, MatchOp::Contains, "x").matches(&ada));
        let no_mail = UserRepresentation {
            email: None,
            ..ada.clone()
        };
        assert!(!m(PersonField::Email, MatchOp::StartsWith, "a").matches(&no_mail));
    }

    #[test]
    fn a_group_matches_by_id_or_name_ignoring_case() {
        let g = GroupRepresentation {
            id: "g-7".into(),
            name: "Traffic Team".into(),
            ..GroupRepresentation::default()
        };
        assert!(m(GroupField::Id, MatchOp::Equals, "G-7").matches(&g));
        assert!(m(GroupField::Name, MatchOp::Contains, "fic t").matches(&g));
        assert!(!m(GroupField::Name, MatchOp::StartsWith, "team").matches(&g));
        assert!(!m(GroupField::Id, MatchOp::Contains, "8").matches(&g));
    }

    #[test]
    fn an_empty_page_holds_nothing() {
        let page: ListPage<u8> = ListPage::default();
        assert!(page.items.is_empty());
        assert_eq!(page.total, 0);
    }
}
