// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::fake_tests::block_on;
use crate::queue::{MOST_QUEUED, Queue};
use crate::{Event, NotificationRecord, NotificationTopic};

fn notice(key: u64) -> Event {
    Event::Notification { notification: NotificationRecord { key, topic: NotificationTopic::Mail, title: "t".into(), body: "b".into() } }
}

fn drain(queue: &std::sync::Arc<Queue<Event>>) -> Vec<Event> {
    (0..queue.len()).filter_map(|_| block_on(queue.next())).collect()
}

#[test]
fn a_flood_of_notifications_keeps_the_sign_in_and_the_question() {
    let queue = Queue::<Event>::new();
    queue.push(Event::SignInNeeded { account: 3 });
    queue.push(Event::PermissionQuestion { account: 3, question: "To Do".into() });
    for key in 0..(MOST_QUEUED as u64 * 3) {
        queue.push(notice(key));
        queue.push(Event::ReminderScheduleChanged);
    }
    assert_eq!(queue.len(), MOST_QUEUED);
    let events = drain(&queue);
    assert!(events.contains(&Event::SignInNeeded { account: 3 }));
    assert!(events.contains(&Event::PermissionQuestion { account: 3, question: "To Do".into() }));
}

#[test]
fn the_oldest_notification_goes_first() {
    let queue = Queue::<Event>::new();
    for key in 0..MOST_QUEUED as u64 {
        queue.push(notice(key));
    }
    queue.push(notice(9_999));
    let events = drain(&queue);
    assert_eq!(events.len(), MOST_QUEUED);
    assert!(!events.contains(&notice(0)));
    assert!(events.contains(&notice(9_999)));
}

#[test]
fn a_notification_never_evicts_a_full_queue_of_sign_ins() {
    let queue = Queue::<Event>::new();
    for account in 0..MOST_QUEUED as u64 {
        queue.push(Event::SignInNeeded { account });
    }
    queue.push(notice(1));
    let events = drain(&queue);
    assert_eq!(events.len(), MOST_QUEUED);
    assert!(!events.contains(&notice(1)));
    for account in 0..MOST_QUEUED as u64 {
        assert!(events.contains(&Event::SignInNeeded { account }));
    }
}
