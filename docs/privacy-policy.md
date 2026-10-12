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
| `openid`, `email` | Learn which address you signed in with |
| `gmail.modify` | Read your mail, mark messages read/unread, and (as features are added) move, label, delete and send |
| `calendar` | Read and (as features are added) edit your calendar events |
| `contacts` | Read and (as features are added) edit your contacts |
| `tasks` | Read and (as features are added) edit your tasks |

**Microsoft accounts (Microsoft 365, Exchange, Outlook.com)**, through the Microsoft Graph API:
mail, calendar, contacts and tasks read/write access and basic profile (`User.Read`), plus
`offline_access` so you stay signed in.

## Where your data is stored

Everything is stored locally, under your own user account on your own computer:

- **Sign-in tokens** are kept in your operating system's secret store (the Secret Service
  keyring, for example GNOME Keyring). Your account password is never seen or stored by the app:
  you enter it on Google's or Microsoft's own sign-in page in your browser.
- **A cache of your mail, calendar, contacts and tasks** is kept in a local SQLite database in
  `~/.config/omalook/` so the app is fast and works offline.

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

The app connects only to the services needed for the accounts you add: Google
(`accounts.google.com`, `oauth2.googleapis.com`, `gmail.googleapis.com`, `www.googleapis.com`,
`people.googleapis.com`) and Microsoft (`login.microsoftonline.com`, `graph.microsoft.com`).
Remote images inside email messages are blocked by default; if you choose to load them, your
device fetches them from whoever hosts them, as in any mail client.

The app contains no analytics, telemetry or crash reporting.

## Retention and deletion

- **Remove an account** in Settings → Accounts to delete its sign-in token from your keyring and
  all of its cached mail, calendar and contact data from the local database. **Log out** keeps the
  token and cache so you can sign back in instantly.
- **Revoke the app's access** at any time from your provider:
  Google: https://myaccount.google.com/permissions ·
  Microsoft: https://account.microsoft.com/privacy/app-access (personal accounts) or
  https://myapps.microsoft.com (work or school accounts).
- **Uninstall completely** by deleting the `~/.config/omalook/` directory and the
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
