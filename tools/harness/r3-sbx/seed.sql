-- Fixture for the read-path comparison (old vs new backend). Applied after the warm-up GET /accounts,
-- so every table exists. Ids are stable on purpose: reqs-read.json points at them.
-- Accounts: acct-a = exchange a@example.com, acct-b = gmail b@example.com.
BEGIN;

INSERT INTO accounts (id, provider, email, display_name, config, enabled) VALUES
 ('acct-a', 'exchange', 'a@example.com', 'Adam (Work)', '{}', 1),
 ('acct-b', 'gmail', 'b@example.com', 'Adam (Personal)', '{}', 1);

INSERT INTO identities (account_id, email, name, is_primary, is_default, fetched_at) VALUES
 ('acct-a', 'a@example.com', 'Adam Work', 1, 1, 1794528000),
 ('acct-b', 'b@example.com', 'Adam Personal', 1, 1, 1794528000);
INSERT INTO identity_pref (account_id, email) VALUES ('acct-a', 'a@example.com');

-- Folders: well_known_name drives the sidebar and the all:<kind> filters.
INSERT INTO folders (id, display_name, parent_folder_id, unread_item_count, total_item_count, well_known_name, sort_order, account_id) VALUES
 ('fa-inbox',   'Inbox',         NULL, 1, 3, 'inbox',        1, 'acct-a'),
 ('fa-sent',    'Sent Items',    NULL, 0, 1, 'sentitems',    2, 'acct-a'),
 ('fa-drafts',  'Drafts',        NULL, 0, 0, 'drafts',       3, 'acct-a'),
 ('fa-deleted', 'Deleted Items', NULL, 0, 0, 'deleteditems', 4, 'acct-a'),
 ('fa-proj',    'Project X',     NULL, 0, 1, NULL,           9, 'acct-a'),
 ('fb-inbox',   'INBOX',         NULL, 1, 2, 'inbox',        1, 'acct-b'),
 ('fb-sent',    'Sent',          NULL, 0, 0, 'sentitems',    2, 'acct-b'),
 ('fb-drafts',  'Drafts',        NULL, 0, 0, 'drafts',       3, 'acct-b'),
 ('fb-trash',   'Trash',         NULL, 0, 0, 'deleteditems', 4, 'acct-b'),
 ('fb-family',  'Family',        NULL, 0, 1, NULL,           9, 'acct-b');

-- Messages. Columns beyond the originals (folder_id, to_text, categories, ...) are filled where the list uses them.
INSERT INTO messages (id, subject, from_email, from_name, body, received_at, is_read, folder_id, read_pending,
                      to_text, cc_text, bcc_text, categories, categories_synced, cat_pending, sent_at, importance,
                      has_attachments, conversation_id, meta_ok, action_pending, action_due, body_full, body_type, account_id) VALUES
 ('msg-a1', 'Q4 planning agenda', 'alice@contoso.com', 'Alice Example',
  'Hi Adam, attached is the agenda for Thursday. Can you review the budget section before we meet?',
  '2026-11-13T14:05:00Z', 0, 'fa-inbox', 0, 'a@example.com', '', '', '["Project X"]', '["Project X"]', 0,
  '2026-11-13T14:05:00Z', 'high', 1, 'conv-1', 1, NULL, 0,
  'Hi Adam,\n\nAttached is the agenda for Thursday. Can you review the budget section before we meet?\n\nAlice', 'text', 'acct-a'),
 ('msg-a2', 'Weekly digest: Rust 1.99', 'news@example.com', 'Weekly Digest',
  'This week in Rust: new release notes, three crate updates and a conference CFP.',
  '2026-11-12T07:30:00Z', 1, 'fa-inbox', 0, 'a@example.com', '', '', '[]', '[]', 0,
  '2026-11-12T07:30:00Z', 'normal', 0, 'conv-2', 1, NULL, 0,
  '<p>This week in Rust: new release notes, three crate updates and a conference CFP.</p>', 'html', 'acct-a'),
 ('msg-a3', 'Dan Dev review request', 'dan@contoso.com', 'Dan Dev',
  'Could you look at PR 412 when you get a moment? It touches the sync worker.',
  '2026-11-11T16:20:00Z', 0, 'fa-inbox', 0, 'a@example.com', 'carol@example.com', '', '[]', '[]', 0,
  '2026-11-11T16:20:00Z', 'normal', 0, 'conv-3', 1, NULL, 0,
  'Could you look at PR 412 when you get a moment? It touches the sync worker.', 'text', 'acct-a'),
 ('msg-a4', 'Re: budget numbers', 'a@example.com', 'Adam Work',
  'Sending the revised numbers now. Shout if the travel line looks off.',
  '2026-11-10T09:12:00Z', 1, 'fa-sent', 0, 'bob@example.com', '', '', '[]', '[]', 0,
  '2026-11-10T09:12:00Z', 'normal', 0, 'conv-4', 1, NULL, 0,
  'Sending the revised numbers now. Shout if the travel line looks off.', 'text', 'acct-a'),
 ('msg-a5', 'Project X kickoff', 'carol@example.com', 'Carol Lee',
  'Kickoff notes and the owner list are in the shared folder.',
  '2026-11-09T11:00:00Z', 1, 'fa-proj', 0, 'a@example.com', '', '', '["Project X"]', '["Project X"]', 0,
  '2026-11-09T11:00:00Z', 'normal', 0, 'conv-5', 1, NULL, 0,
  'Kickoff notes and the owner list are in the shared folder.', 'text', 'acct-a'),
 ('msg-b1', 'Dinner Sunday?', 'mom@gmail.com', 'Mom',
  'Are you coming Sunday? I am making the lasagna.',
  '2026-11-13T18:44:00Z', 0, 'fb-inbox', 0, 'b@example.com', '', '', '["Family"]', '["Family"]', 0,
  '2026-11-13T18:44:00Z', 'normal', 0, 'conv-6', 1, NULL, 0,
  'Are you coming Sunday? I am making the lasagna.', 'text', 'acct-b'),
 ('msg-b3', 'Photos from the lake', 'mom@gmail.com', 'Mom',
  'Uploaded the lake photos to the family album, the one with the dog is my favourite.',
  '2026-11-05T20:10:00Z', 1, 'fb-family', 0, 'b@example.com', '', '', '["Family"]', '["Family"]', 0,
  '2026-11-05T20:10:00Z', 'normal', 0, 'conv-8', 1, NULL, 0,
  'Uploaded the lake photos to the family album, the one with the dog is my favourite.', 'text', 'acct-b'),
 ('msg-b2', 'Your invoice #7731', 'billing@shop.example', 'Shop Billing',
  'Invoice 7731 for 42.00 EUR is attached.',
  '2026-11-08T08:00:00Z', 1, 'fb-inbox', 0, 'b@example.com', '', '', '[]', '[]', 0,
  '2026-11-08T08:00:00Z', 'low', 1, 'conv-7', 1, NULL, 0,
  'Invoice 7731 for 42.00 EUR is attached.', 'text', 'acct-b');

-- Folder-level delta state for the two Inboxes (as if the first full sync finished).
INSERT INTO mail_delta (account_id, folder_id, link, complete) VALUES
 ('acct-a', 'fa-inbox', 'delta-token-a', 1),
 ('acct-b', 'fb-inbox', 'delta-token-b', 1);

-- Attachment cache for the message that has attachments (message opened earlier).
INSERT INTO attachment_cache (message_id, json, has_real, fetched_at) VALUES
 ('msg-a1', '[{"id":"att-1","name":"agenda.pdf","size":48213,"content_type":"application/pdf","inline":false,"kind":"file"}]', 1, 1794528000);

-- Categories per account, with colours. Gmail labels are the same table.
INSERT INTO category_defs (account_id, name, color, provider_color, pending, color_push, delete_pending) VALUES
 ('acct-a', 'Project X', '#4a90d9', '#4a90d9', 0, 0, 0),
 ('acct-b', 'Family', '#e67e22', NULL, 0, 0, 0);
INSERT INTO category_state (account_id, master_list) VALUES ('acct-a', 1), ('acct-b', 1);

INSERT INTO sender_prefs (email, always_html, always_images, created_at) VALUES
 ('news@example.com', 0, 1, '2026-11-01 08:00:00');

-- Contacts (Graph-shaped JSON in `data`; list fields as JSON arrays). search_text is the lower-cased text the People search matches.
INSERT INTO contact_folders (id, display_name, parent_folder_id, account_id) VALUES
 ('cf-a-contacts', 'Contacts', NULL, 'acct-a'),
 ('cf-b-contacts', 'Contacts', NULL, 'acct-b');

INSERT INTO contacts (id, display_name, given_name, surname, company, job_title, emails, phones, addresses, folder_id,
                      created_at, modified_at, is_favorite, data, change_key, categories, sync_state, search_text, account_id) VALUES
 ('contact-alice', 'Alice Example', 'Alice', 'Example', 'Contoso', 'Program Manager',
  '["alice@contoso.com"]', '[{"kind":"work","number":"+1 555 0101"}]',
  '[{"kind":"work","text":"1 Main St, Springfield"}]', 'cf-a-contacts',
  '2026-01-05T10:00:00Z', '2026-10-30T10:00:00Z', 1,
  '{"id":"contact-alice","displayName":"Alice Example","givenName":"Alice","surname":"Example","companyName":"Contoso","jobTitle":"Program Manager","emailAddresses":[{"name":"Alice Example","address":"alice@contoso.com"}],"businessPhones":["+1 555 0101"],"categories":["Project X"]}',
  'ck-alice-1', '["Project X"]', 'ok',
  'alice example alice example alice@contoso.com +1 555 0101 contoso program manager project x', 'acct-a'),
 ('contact-bob', 'Bob Builder', 'Bob', 'Builder', 'Builder Co', '',
  '["bob@example.com"]', '[{"kind":"mobile","number":"+1 555 0102"}]', '[]', 'cf-a-contacts',
  '2026-02-11T09:00:00Z', '2026-09-02T09:00:00Z', 0,
  '{"id":"contact-bob","displayName":"Bob Builder","givenName":"Bob","surname":"Builder","companyName":"Builder Co","emailAddresses":[{"name":"Bob Builder","address":"bob@example.com"}],"mobilePhone":"+1 555 0102","categories":[]}',
  'ck-bob-1', '[]', 'ok', 'bob builder bob builder bob@example.com +1 555 0102 builder co', 'acct-a'),
 ('contact-mom', 'Mom', 'Mom', '', '', '',
  '["mom@gmail.com"]', '[{"kind":"mobile","number":"+1 555 0199"}]', '[]', 'cf-b-contacts',
  '2025-12-24T12:00:00Z', '2026-11-13T18:44:00Z', 0,
  '{"id":"contact-mom","displayName":"Mom","emailAddresses":[{"name":"Mom","address":"mom@gmail.com"}],"mobilePhone":"+1 555 0199","categories":["Family"]}',
  'ck-mom-1', '["Family"]', 'ok', 'mom mom@gmail.com +1 555 0199 family', 'acct-b'),
 ('contact-dan', 'Dan Dev', 'Dan', 'Dev', 'Contoso', 'Engineer',
  '["dan@contoso.com"]', '[]', '[]', 'cf-b-contacts',
  '2026-03-01T08:00:00Z', '2026-06-15T08:00:00Z', 0,
  '{"id":"contact-dan","displayName":"Dan Dev","givenName":"Dan","surname":"Dev","companyName":"Contoso","jobTitle":"Engineer","emailAddresses":[{"name":"Dan Dev","address":"dan@contoso.com"}],"categories":[]}',
  'ck-dan-1', '[]', 'ok', 'dan dev dan@contoso.com contoso engineer', 'acct-b');

-- Distribution list "Team" (local lists, shown in the Lists view). member_key = lower-cased address.
INSERT INTO dl_lists (id, name, name_key, description, created_at, updated_at) VALUES
 (1, 'Team', 'team', 'Project X people', 1794528000, 1794528000);
INSERT INTO dl_members (list_id, member_key, address, display_name, added_at) VALUES
 (1, 'alice@contoso.com', 'alice@contoso.com', 'Alice Example', 1794528000),
 (1, 'dan@contoso.com', 'dan@contoso.com', 'Dan Dev', 1794528000);

-- Calendar events. calendar_id groups them per account; start/end are local wall-clock strings.
INSERT INTO calendar_events (id, subject, body, start_at, end_at, is_all_day, time_zone, event_type, series_master_id,
                             location, organizer_name, organizer_email, is_organizer, attendees_json, my_response, show_as,
                             online_provider, online_url, reminder_minutes, web_link, sensitivity, recurrence_json,
                             calendar_id, tags_json, account_id) VALUES
 ('evt-review', 'Design review', 'Walk through the Q4 budget section.', '2026-11-16T10:00:00', '2026-11-16T11:00:00', 0,
  'America/New_York', 'singleInstance', NULL, 'Room 4B', 'Adam Work', 'a@example.com', 1,
  '[{"name":"Alice Example","email":"alice@contoso.com","response":"accepted","type":"required"}]',
  'organizer', 'busy', 'teams', 'https://teams.example/meet/review', 15, '', 'normal', '',
  'cal-a-main', '["Project X"]', 'acct-a'),
 ('evt-standup', 'Standup', '', '2026-11-02T09:00:00', '2026-11-02T09:15:00', 0,
  'America/New_York', 'seriesMaster', NULL, '', 'Adam Work', 'a@example.com', 1, '[]',
  'organizer', 'busy', '', '', NULL, '', 'normal',
  '{"pattern":{"type":"weekly","interval":1,"daysOfWeek":["monday"]},"range":{"type":"noEnd","startDate":"2026-11-02"}}',
  'cal-a-main', '[]', 'acct-a'),
 ('evt-standup-1116', 'Standup', '', '2026-11-16T09:00:00', '2026-11-16T09:15:00', 0,
  'America/New_York', 'occurrence', 'evt-standup', '', 'Adam Work', 'a@example.com', 1, '[]',
  'organizer', 'busy', '', '', NULL, '', 'normal', '',
  'cal-a-main', '[]', 'acct-a'),
 ('evt-birthday', 'Mom''s birthday', 'Dinner at home.', '2026-11-20T00:00:00', '2026-11-21T00:00:00', 1,
  'America/New_York', 'singleInstance', NULL, '', 'Mom', 'mom@gmail.com', 0, '[]',
  'accepted', 'free', '', '', 1440, '', 'normal', '',
  'cal-b-main', '["Family"]', 'acct-b');

-- Tasks: one list per account.
INSERT INTO task_lists (id, account_id, name, is_default) VALUES
 ('tl-a-default', 'acct-a', 'Tasks', 1),
 ('tl-b-default', 'acct-b', 'My Tasks', 1);
INSERT INTO tasks (id, account_id, list_id, title, notes, completed, due, importance, modified_at, completed_at) VALUES
 ('task-1', 'acct-a', 'tl-a-default', 'Send budget deck', 'Final numbers from Bob', 0, '2026-11-18', 'high', '2026-11-12T10:00:00Z', ''),
 ('task-2', 'acct-a', 'tl-a-default', 'Book Room 4B', '', 1, '', 'normal', '2026-11-09T12:00:00Z', '2026-11-09T12:00:00Z'),
 ('task-3', 'acct-b', 'tl-b-default', 'Buy birthday gift', '', 0, '2026-11-19', 'normal', '2026-11-13T19:00:00Z', '');

-- Local draft (not yet on the provider).
INSERT INTO drafts (local_id, account_id, remote_id, remote_version, kind, in_reply_to, payload_json, mode,
                    attachments_sig, dirty, state, attempts, next_attempt_at, last_error, notice, created_at, updated_at) VALUES
 ('draft-local-1', 'acct-a', NULL, NULL, 'new', '',
  '{"kind":"new","account_id":"acct-a","in_reply_to":"","to":[{"email":"bob@example.com","name":"Bob Builder"}],"cc":[],"bcc":[],"subject":"Budget numbers v2","from":"","mode":"system","body":"Bob, here is the second pass.","attachments":[]}',
  'system', '', 1, 'editing', 0, 0, NULL, NULL, 1794528000, 1794528300);

-- Rules. Exchange rule in Graph messageRule shape; Gmail has no server rules in this fixture.
INSERT INTO mail_rules (account_id, rule_id, sequence, raw) VALUES
 ('acct-a', 'rule-news', 1,
  '{"id":"rule-news","displayName":"Move newsletters","sequence":1,"isEnabled":true,"hasError":false,"conditions":{"fromAddresses":[{"emailAddress":{"address":"news@example.com"}}]},"actions":{"moveToFolder":"fa-proj","stopProcessingRules":false}}');
INSERT INTO rules_state (account_id, state, error, fetched_at) VALUES ('acct-a', 'ok', '', 1794528000);

-- Local rule (tags on arrival, the rule engine's own JSON form). Pred serde: {"kind":..., "value":...}.
INSERT INTO local_rules (id, sequence, json) VALUES
 ('lr-1', 10,
  '{"id":"lr-1","name":"Tag Carol as Project X","enabled":true,"sequence":10,"applies_to":[],"conditions":[{"kind":"from_addresses","value":["carol@example.com"]}],"exceptions":[],"actions":{"tags":["Project X"],"stop":false}}');
INSERT INTO local_rule_stats (rule_id, applied, last_applied_at) VALUES ('lr-1', 1, 1794528000);
INSERT INTO local_rules_state (account_id, since) VALUES ('acct-a', '2026-11-01T00:00:00'), ('acct-b', '2026-11-01T00:00:00');

-- SMS: one paired device, two threads.
INSERT INTO sms_devices (device_id, name, full_synced_at, last_seen_at) VALUES
 ('dev-1', 'Pixel 8', '2026-11-13 12:00:00', '2026-11-13 18:00:00');
INSERT INTO sms_threads (device_id, thread_id, history_complete, phone_count) VALUES
 ('dev-1', 101, 1, 1), ('dev-1', 102, 1, 1);
INSERT INTO sms_messages (device_id, mms, uid, thread_id, body, addresses, date_ms, direction, event, sub_id, attachments, is_read, phone_read) VALUES
 ('dev-1', 0, 1001, 101, 'Are we still on for Sunday?', '["+15555550101"]', 1794578700000, 0, 0, 1, '[]', 1, 1),
 ('dev-1', 0, 1002, 101, 'Yes, bringing dessert.', '["+15555550101"]', 1794579000000, 1, 0, 1, '[]', 1, 1),
 ('dev-1', 0, 1003, 102, 'Your pickup is ready.', '["+15555550102"]', 1794500000000, 0, 0, 1, '[]', 0, 0);

COMMIT;
