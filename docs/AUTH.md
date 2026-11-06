# Microsoft sign-in

Kestrel uses the OAuth 2.0 **device-code flow** for Microsoft accounts. There is
no embedded login webview, no redirect URI, and no client secret. The launcher
shows a short code; you approve it at `https://microsoft.com/link`; Kestrel
polls Microsoft and then exchanges the result for an Xbox Live, XSTS and
Minecraft session.

## 1. Create an Azure application

You only need to do this once, for your own build of Kestrel.

1. Open the [Azure portal](https://portal.azure.com) → **Microsoft Entra ID** →
   **App registrations** → **New registration**.
2. Name it anything (for example `Kestrel`).
3. Supported account types: **Personal Microsoft accounts only**.
4. Leave **Redirect URI** empty.
5. After creating it, open **Authentication** → **Advanced settings** and set
   **Allow public client flows** to **Yes**. Save.
6. Copy the **Application (client) ID** from the overview page.

No certificate, secret, or API permission is required. The device-code flow
uses the `XboxLive.signin offline_access` scope.

## 2. Point Kestrel at the client ID

Either export it:

```bash
export KESTREL_AZURE_CLIENT_ID=00000000-0000-0000-0000-000000000000
```

or put it in `~/.config/kestrel/config.json` (or
`$KESTREL_CONFIG_DIR/config.json`):

```json
{
  "azureClientId": "00000000-0000-0000-0000-000000000000"
}
```

The environment variable wins over the file. Restart the launcher after
changing it.

## 3. Sign in

Click **Sign in**, then **Continue with Microsoft**. The launcher displays a
code such as `ABCD-EFGH` and a link. Open the link, enter the code, and approve
the request. The window updates automatically when the token chain completes.

## What is stored where

- `StoredTokens` (Microsoft refresh/access token, Minecraft access token,
  profile UUID/name/XUID) go into the **system keyring** via Secret Service:
  service `dev.kestrel.launcher`, user `microsoft`.
- `state.json` stores only the public account summary (name and UUID).
- The OAuth `device_code` stays in Rust memory for the duration of the attempt.
  It is marked `#[serde(skip_serializing)]`, so the webview and logs never see
  it.
- Tokens are never written to logs and never sent anywhere except Microsoft,
  Xbox Live and Mojang APIs.

On startup the saved session is validated against the Minecraft profile
endpoint on a background task after the first frame. A token close to expiry is
refreshed first. If Microsoft invalidates the refresh token, the keyring entry
is removed and the UI asks you to sign in again.

## Troubleshooting

| Message | Meaning / fix |
|---|---|
| `AADSTS700038: … is not a valid application identifier` | Wrong or placeholder client ID. Copy the Application (client) ID again. |
| `AADSTS700016: Application … was not found` | The app belongs to a different tenant, or the ID is for a confidential client. Use a personal-accounts public client. |
| `AADSTS70008` / `invalid_grant` | The refresh token expired or was revoked. Sign in again. |
| `This Microsoft account has no Xbox profile` (XErr 2148916233) | Sign in once at `xbox.com`, choose a gamertag, then retry. |
| `Xbox Live is not available in this account's country` (2148916235) | Xbox Live is unsupported in the account region. |
| `needs adult verification` (2148916236/37) | Complete Microsoft's adult verification for the account. |
| `child account` (2148916238) | Add the account to a Microsoft family group. |
| `does not own Minecraft: Java Edition` | The account has no Java profile. Kestrel does not support offline/cracked accounts. |
| `keyring is unavailable` | Start a Secret Service provider (GNOME Keyring, KWallet with the Secret Service bridge, or `gnome-keyring-daemon`). |

## Development-only bypass

`KESTREL_DEV_AUTH=1 kestrel dev-launch <version>` uses a placeholder token to
smoke-test the launch pipeline. It is deliberately gated by an environment
variable, is never persisted, and cannot join online servers. It is not an
offline-mode account.
