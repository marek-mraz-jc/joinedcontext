//! API/01 §35, T-3106: comments and notifications on PostgreSQL. A comment notifies each mentioned
//! person once, its author removes it with its notifications, and a notification is read by its
//! recipient alone. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may
//! write to; otherwise it says so and returns, like the other database suites.

use joinedcontext_portal::entity_comments::{
    approvers_of, Author, CommentStore, NewComment, Notice,
};

#[tokio::test]
async fn a_comment_notifies_its_mentions_and_leaves_with_its_notifications() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let store = CommentStore::new(Some(pool));
    let project = format!(
        "comments-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let anna = format!("anna@{project}.example");
    let pia = format!("pia@{project}.example");
    let mentions = vec![anna.clone(), pia.clone()];
    let urn = "urn:ngsi-ld:T:x:air:1";
    let made = store
        .add(
            Author {
                id: "sami",
                name: "Sami",
                subject: "sub-sami",
            },
            NewComment {
                project: &project,
                space: "air",
                urn,
                text: "pm10 jumped",
                mentions: &mentions,
            },
        )
        .await
        .expect("stored")
        .expect("room for it");
    assert_eq!(made.mentions, mentions);
    store
        .add(
            Author {
                id: "anna",
                name: "Anna",
                subject: "sub-anna",
            },
            NewComment {
                project: &project,
                space: "air",
                urn,
                text: "seen",
                mentions: &[],
            },
        )
        .await
        .expect("stored");

    let listed = store.list(&project, "air", urn).await.expect("listed");
    assert_eq!(
        listed.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
        ["pm10 jumped", "seen"]
    );
    assert!(store
        .list(&project, "other", urn)
        .await
        .expect("listed")
        .is_empty());

    let (inbox, unread) = store
        .notifications(std::slice::from_ref(&anna))
        .await
        .expect("read");
    assert_eq!((inbox.len(), unread), (1, 1));
    assert_eq!(inbox[0].comment_id, Some(made.id));
    assert!(!store
        .mark_read(std::slice::from_ref(&pia), inbox[0].id)
        .await
        .expect("asked"));
    assert!(store
        .mark_read(std::slice::from_ref(&anna), inbox[0].id)
        .await
        .expect("asked"));
    assert_eq!(
        store
            .notifications(std::slice::from_ref(&anna))
            .await
            .expect("read")
            .1,
        0
    );

    // another person, and a later account that took the username `sami`, remove nothing
    for subject in ["sub-anna", "sami", "sub-sami-recreated"] {
        assert!(!store
            .remove(&project, "air", subject, made.id)
            .await
            .expect("asked"));
    }
    assert_eq!(listed[0].author_subject.as_deref(), Some("sub-sami"));
    assert!(store
        .remove(&project, "air", "sub-sami", made.id)
        .await
        .expect("asked"));
    assert!(
        store
            .notifications(&[pia])
            .await
            .expect("read")
            .0
            .is_empty(),
        "the notification left with its comment"
    );
}

/// T-5979: a connector event is one row per recipient, an audience notice reaches a person once
/// when they read, never the person who caused it, and carries its link.
#[tokio::test]
async fn a_connector_event_reaches_each_reader_of_its_audience_once() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let store = CommentStore::new(Some(pool));
    let project = format!(
        "notices-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let audience = approvers_of(&project);
    let link = format!("/projects/{project}/approvals/cr-1");
    let key = format!("change.opened:{project}/cr-1");
    let notice = || Notice {
        kind: "change.opened",
        project: &project,
        about: "cr-1",
        author: "sami",
        author_name: "Sami",
        connector: "doprava-in-claude",
        link: &link,
        key: &key,
    };
    assert!(store.notify(&audience, notice()).await.expect("stored"));
    assert!(!store.notify(&audience, notice()).await.expect("stored"));
    let anna = format!("anna@{project}.example");
    let audiences = vec![audience.clone()];
    for _ in 0..2 {
        store
            .deliver(&anna, std::slice::from_ref(&anna), &audiences)
            .await
            .expect("delivered");
    }
    let (inbox, unread) = store
        .notifications(std::slice::from_ref(&anna))
        .await
        .expect("listed");
    assert_eq!((inbox.len(), unread), (1, 1));
    assert_eq!(inbox[0].kind, "change.opened");
    assert_eq!(inbox[0].link.as_deref(), Some(link.as_str()));
    assert_eq!(inbox[0].comment_id, None);
    // The author reads nothing of their own event; a person outside the audience nothing at all.
    let sami = vec!["sami".to_owned()];
    store
        .deliver("sami", &sami, &audiences)
        .await
        .expect("delivered");
    assert!(store
        .notifications(&sami)
        .await
        .expect("listed")
        .0
        .is_empty());
    let pia = vec![format!("pia@{project}.example")];
    store.deliver(&pia[0], &pia, &[]).await.expect("delivered");
    assert!(store
        .notifications(&pia)
        .await
        .expect("listed")
        .0
        .is_empty());
}
