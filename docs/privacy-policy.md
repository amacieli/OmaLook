# omalook Privacy Policy

**Effective date:** October 4, 2026
**Applies to:** the OmaLook desktop application ("omalook", "the app")
**Source code:** https://github.com/amacieli/OmaLook

## Summary

omalook is a desktop mail, calendar, contacts and tasks client. It runs entirely on your
computer. There is no omalook server: the app talks directly from your machine to your mail
provider (Google or Microsoft), and what it downloads stays on your machine. The developer does
not receive, see, store or sell your data.

## What the app accesses

When you sign in to an account, you grant the app the permissions below. The app requests only
what its features need.

**Google accounts (Gmail, Google Calendar, Google Contacts, Google Tasks)**

| Permission (OAuth scope) | Why |
|---|---|
| `openid`, `email` | Learn which address you signed in with (read from Google's userinfo endpoint) |
| `gmail.modify` | Read your mail, mark messages read/unread, and (as features are added) move, label, delete and send |
| `gmail.settings.basic` | Read and manage your Gmail filters (mail rules). This is a Google "restricted" scope |
| `calendar` | Read and (as features are added) edit your calendar events |
| `contacts` | Read and (as features are added) edit your contacts |
| `tasks` | Read and (as features are added) edit your tasks |

**Microsoft accounts (Microsoft 365, Exchange, Outlook.com)**, through the Microsoft Graph API:

| Permission (delegated scope) | Why |
|---|---|
| `Mail.ReadWrite` | Read your mail and change it (read state, move, delete, drafts) |
| `Mail.Send` | Send mail you compose |
| `Calendars.ReadWrite` | Read and edit your calendar events |
| `Contacts.ReadWrite` | Read and edit your contacts |
| `Tasks.ReadWrite` | Read and edit your tasks |
| `MailboxSettings.ReadWrite` | Read and edit your mail category list (names and colours) |
| `User.Read` | Basic profile: your address and display name |
| `offline_access` | Stay signed in (refresh token) |

If an account has not granted the newer write permissions, the app falls back to read-only
calendar and contacts access (`Calendars.Read`, `Contacts.Read`) instead of signing you out.

## Where your data is stored

Everything is stored locally, under your own user account on your own computer:

- **Sign-in tokens** are kept in your operating system's secret store (the Secret Service
  keyring, for example GNOME Keyring). Your account password is never seen or stored by the app:
  you enter it on Google's or Microsoft's own sign-in page in your browser.
- **A cache of your mail, calendar, contacts and tasks** is kept in a local SQLite database in
  `~/.config/omalook/` so the app is fast and works offline. Rendered message images, drafts,
  outgoing mail and contact photos are cached in `~/.cache/omalook/`.

**Encryption at rest.** The database and cache files are not encrypted by the app. They are
protected by file permissions only: the directories are mode 0700 and the files 0600, so other
user accounts on the computer cannot read them. Anyone with access to the disk itself, or to your
logged-in session, can. If that matters to you, use full-disk encryption (for example LUKS). Only
the sign-in tokens are held in the system keyring, which is encrypted by your login.

Nothing is uploaded to the developer or to any third party by the app.

## How the data is used

Only to show it to you and carry out the actions you request in the app (for example marking a
message read, which is sent to Google or Microsoft). The app does not use your data for
advertising, profiling, analytics or machine-learning training, and it does not sell, rent or
share it.

## Google API Services User Data Policy

omalook's use and transfer to any other app of information received from Google APIs will
adhere to the
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
including the Limited Use requirements. In particular:

- Data from Google APIs is used only to provide and improve the user-facing features of the app.
- It is not transferred to others except as needed to provide those features, to comply with law,
  or as part of a merger or sale with your consent.
- It is not used for serving advertisements.
- No human reads your data: the developer has no access to it, since it never leaves your device.

## Network connections the app makes

The app connects only to the services needed for the accounts you add:

| Host | Used for |
|---|---|
| `accounts.google.com` | Google sign-in page (opened in your browser) |
| `oauth2.googleapis.com` | Google token exchange and refresh |
| `openidconnect.googleapis.com` | Google userinfo: the address of the account that signed in |
| `gmail.googleapis.com` | Gmail mail, send and filters |
| `www.googleapis.com` | Google Calendar and Google Tasks |
| `people.googleapis.com` | Google Contacts |
| `login.microsoftonline.com` | Microsoft sign-in (device code) and token refresh |
| `graph.microsoft.com` | Microsoft mail, calendar, contacts, tasks and profile |

The app does not fetch Jitsi, Zoom, Teams or any other meeting service itself; see the next
section. Remote images inside email messages are blocked by default; if you choose to load them, your
device fetches them from whoever hosts them, as in any mail client.

The app contains no analytics, telemetry or crash reporting.

### Hosts contacted only when you click

These are opened in your default browser or handler, not fetched by the app, and only when you
activate them:

- **Meeting join links** in calendar events and mail (Join button): the link's own host. The app
  recognises `teams.microsoft.com`, `teams.live.com`, `zoom.us` (and subdomains), `meet.google.com`,
  `webex.com` (and subdomains) and `meet.jit.si`. Links to other hosts can also be offered as a
  plain link.
- **Online meetings you create**: a Teams or Google Meet link is created by Microsoft or Google
  through the account's own API. A Jitsi link is generated on your computer, by default on
  `meet.jit.si` or on a server you set in Settings; your personal Zoom link is whatever `https://`
  address you enter. Nothing is contacted until you or an invitee opens it.
- **Links in mail** and in contact details: any `http`, `https` or `mailto` address, opened with
  your system handler.
- **Addresses in contacts**: "open address" opens `www.openstreetmap.org` with the address text
  in the search query.

That host then sees your IP address and whatever the link contains, as with any link you click.

## Retention and deletion

- **Remove an account** in Settings → Accounts stops its sync, deletes its sign-in token from
  your keyring, and deletes from the local database everything stored for it: messages (including
  the full-text search index and the attachment, invite, mailing-list and rule records that belong
  to them), folders, categories, drafts and outgoing mail, calendar events, contacts, tasks, mail
  rules, sending identities and sync state. Its cached files in `~/.cache/omalook/` are deleted too:
  rendered message images, drafts, outgoing mail and contact photos. Freed database pages are
  overwritten with zeros (SQLite `secure_delete`) during the removal.
  What is **not** removed: text-message (SMS) data and its attachments, which belong to your
  paired phone and not to a mail account; the transient `outgoing-tmp` staging directory; and data
  that earlier deletes or updates already left in free pages or the write-ahead log, which this
  removal does not rewrite. To erase those too, delete `~/.config/omalook/` and `~/.cache/omalook/`
  (see below). **Log out** keeps the token and cache so you can sign back in instantly.
- **Revoke the app's access** at any time from your provider:
  Google: https://myaccount.google.com/permissions ·
  Microsoft: https://account.microsoft.com/privacy/app-access (personal accounts) or
  https://myapps.microsoft.com (work or school accounts).
- **Uninstall completely** by deleting the `~/.config/omalook/` and `~/.cache/omalook/` directories and the
  `omalook` entries in your keyring.

Because the developer holds none of your data, there is nothing for the developer to delete on
your behalf.

## Children

The app is not directed at children under 13 and does not knowingly collect their information.

## Changes to this policy

If this policy changes, the new version will be published at this address with an updated
effective date.

## Contact

Questions about this policy: open an issue at https://github.com/amacieli/OmaLook/issues or
write to adam.macielinski@gmail.com.

**Developer:** Adam Macielinski
