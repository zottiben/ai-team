use super::*;
use crate::{ModelRegistry, NewChat, NewProject, NodeStatus, Provider, Reasoning, Store};

fn png() -> ImageInput {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    ImageInput {
        name: "screenshot.png".into(),
        media_type: "image/png".into(),
        data: STANDARD.encode(bytes.into_inner()),
    }
}

fn fixture() -> (tempfile::TempDir, Store, crate::Chat) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(&dir.path().join("team.db")).unwrap();
    let project = store
        .create_project(NewProject {
            name: "Image fixture".into(),
            ..Default::default()
        })
        .unwrap();
    let chat = store
        .create_chat(NewChat {
            project_id: project.id,
            workspace: dir.path().into(),
            provider: Provider::Local,
            model: "fixture".into(),
            reasoning: Reasoning::High,
        })
        .unwrap();
    (dir, store, chat)
}
fn input() -> Prompt {
    Prompt {
        expected_reasoning: None,
        workspace_epoch: 0,
        images: validate(vec![png()]).unwrap(),
        publication: None,
    }
}

#[test]
fn image_validation_refuses_unsupported_corrupt_mismatched_and_unbounded_input() {
    assert!(validate(vec![png()]).is_ok());
    assert!(validate(vec![png(); 5]).is_err());
    let mut bad = png();
    bad.media_type = "image/svg+xml".into();
    assert!(validate(vec![bad]).is_err());
    let mut bad = png();
    bad.media_type = "image/jpeg".into();
    assert!(validate(vec![bad]).is_err());
    let mut bad = png();
    bad.data = STANDARD.encode(b"\x89PNG\r\n\x1a\ntruncated");
    assert!(validate(vec![bad]).is_err());
    let mut bad = png();
    bad.data = "!not-base64".into();
    assert!(validate(vec![bad]).is_err());
    let mut bad = png();
    bad.data = "A".repeat(MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1);
    assert!(validate(vec![bad]).is_err());
    let mut bad = png();
    bad.name = "bad\nname".into();
    assert!(validate(vec![bad]).is_err());
    let mut wide = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8193, 1)
        .write_to(&mut wide, image::ImageFormat::Png)
        .unwrap();
    let mut bad = png();
    bad.data = STANDARD.encode(wide.into_inner());
    assert!(validate(vec![bad])
        .unwrap_err()
        .to_string()
        .contains("8192px"));
}

#[test]
fn images_are_atomic_scoped_immutable_and_part_of_replay_identity() {
    let (_dir, mut store, chat) = fixture();
    let registry = ModelRegistry::local_only();
    let first = store
        .begin_chat_prompt(chat.id, "Inspect this", "image", &registry, input())
        .unwrap();
    assert!(first.started);
    assert_eq!(
        store.chat_image(chat.id, first.node_id, 0).unwrap().data,
        png().data
    );
    assert!(store.chat_image(chat.id + 1, first.node_id, 0).is_err());
    assert!(store.chat_image(chat.id, first.node_id, 1).is_err());
    let history = store.chat_turns(chat.id).unwrap();
    assert_eq!(history[0].images[0].name, "screenshot.png");
    assert_eq!(
        history[0].images[0].bytes,
        STANDARD.decode(png().data).unwrap().len()
    );
    assert!(!serde_json::to_string(&history)
        .unwrap()
        .contains(&png().data));
    assert!(
        !store
            .begin_chat_prompt(chat.id, "Inspect this", "image", &registry, input())
            .unwrap()
            .started
    );
    assert!(store
        .begin_chat_turn(chat.id, "Inspect this", "image", &registry)
        .is_err());
    let mut changed = png();
    changed.name = "another.png".into();
    assert!(store
        .begin_chat_prompt(
            chat.id,
            "Inspect this",
            "image",
            &registry,
            Prompt {
                images: validate(vec![changed]).unwrap(),
                ..input()
            }
        )
        .is_err());
    assert!(store
        .db_mut()
        .write(|tx| {
            tx.execute(
                "UPDATE chat_turn SET images_json='[]' WHERE node_id=?1",
                [first.node_id],
            )?;
            Ok(())
        })
        .is_err());
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let mut stale = input();
    stale.workspace_epoch = 99;
    assert!(store
        .begin_chat_prompt(chat.id, "other", "stale", &registry, stale)
        .is_err());
    assert_eq!(store.chat_turns(chat.id).unwrap().len(), 1);
}

#[test]
fn queued_images_follow_the_exact_instruction_after_drain() {
    let (_dir, mut store, chat) = fixture();
    let registry = ModelRegistry::local_only();
    let first = store
        .begin_chat_turn(chat.id, "first", "first", &registry)
        .unwrap();
    let images = validate(vec![png()]).unwrap();
    let queued = store
        .queue_chat_followup_with_images(
            chat.id,
            first.node_id,
            "look",
            "queued",
            crate::chat::FollowupKind::FollowUp,
            &images,
        )
        .unwrap();
    assert_eq!(queued.image_count, 1);
    assert!(store
        .queue_chat_followup(
            chat.id,
            first.node_id,
            "look",
            "queued",
            crate::chat::FollowupKind::FollowUp
        )
        .is_err());
    assert!(store
        .send_chat_followup(chat.id, queued.id, &registry)
        .is_err());
    store
        .finish_chat_turn(chat.id, first.node_id, NodeStatus::Done, None)
        .unwrap();
    let delivered = store
        .send_chat_followup(chat.id, queued.id, &registry)
        .unwrap();
    assert!(delivered.started);
    assert_eq!(
        store
            .chat_image(chat.id, delivered.node_id, 0)
            .unwrap()
            .data,
        png().data
    );
    assert!(
        !store
            .send_chat_followup(chat.id, queued.id, &registry)
            .unwrap()
            .started
    );
    acknowledge_images(&mut store, chat.id, queued.id, delivered.node_id);
}

fn acknowledge_images(store: &mut Store, chat: i64, queued: i64, node: i64) {
    let prompt = "<file name=\"/private/generated/0.png\"></file>\nrules\nlook";
    store.prepare_chat_followup_prompt(node, prompt).unwrap();
    let image = png();
    let block = serde_json::json!({"type":"image","mimeType":image.media_type,"data":image.data});
    let event = |role: &str, text: &str, images: Vec<serde_json::Value>| {
        let mut content = vec![serde_json::json!({"type":"text","text":text})];
        content.extend(images);
        crate::PiEvent::parse(
            &serde_json::json!({"type":"message_end","message":{"role":role,"content":content}})
                .to_string(),
        )
        .unwrap()
    };
    let wrong = [
        event("user", prompt, vec![]),
        event("assistant", prompt, vec![block.clone()]),
        event("user", "look", vec![block.clone()]),
        event(
            "user",
            prompt,
            vec![serde_json::json!({"type":"image","mimeType":"image/png","data":"different"})],
        ),
    ];
    for (index, event) in wrong.into_iter().enumerate() {
        store
            .ingest_pi_events(node, "images", i64::try_from(index).unwrap(), &[event])
            .unwrap();
        assert_eq!(store.chat_followup(chat, queued).unwrap().state, "starting");
    }
    store
        .ingest_pi_events(node, "images", 4, &[event("user", prompt, vec![block])])
        .unwrap();
    assert_eq!(
        store.chat_followup(chat, queued).unwrap().state,
        "delivered"
    );
}

#[test]
fn materialized_images_use_private_generated_names_and_are_removed_after_the_turn() {
    let mut image = png();
    image.name = "../../outside.png".into();
    let images = validate(vec![image]).unwrap();
    let directory = materialize(&images.0).unwrap().unwrap();
    let path = directory.path().join("0.png");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        STANDARD.decode(png().data).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    drop(directory);
    assert!(!path.exists());
}
