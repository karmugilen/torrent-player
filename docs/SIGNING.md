# Release signing (GitHub / sideload)

Torrent Player GitHub APKs should be signed with a **dedicated release
keystore**, not the Android Debug key.

## Local secrets (not in git)

On this machine the release key lives outside the repo:

- Keystore: `~/.config/webtor/webtor-release.pkcs12`
- Env file: `~/.config/webtor/credentials.env`

```bash
source ~/.config/webtor/credentials.env
cd android
JAVA_HOME=/usr/lib/jvm/java-21-openjdk ./gradlew :app:assembleRelease --no-daemon
```

Do **not** pass `-Pwebtor.localSigning=true` when the `WEBTOR_*` variables are
set. That flag is only for debug-key local installs.

Required environment variables (also documented in `README.md`):

- `WEBTOR_KEYSTORE`
- `WEBTOR_STORE_PASSWORD`
- `WEBTOR_KEY_ALIAS`
- `WEBTOR_KEY_PASSWORD`

`*.jks` / `*.keystore` are gitignored. Never commit the PKCS12 file or
passwords.

## Verify

```bash
apksigner verify --print-certs -v app/build/outputs/apk/release/app-release.apk
```

Expected signer DN includes `CN=Torrent Player` (not `CN=Android Debug`).

## Updating devices that used the old debug-signed builds

Installs signed with Android Debug **cannot** update in place to this release
key. Uninstall the old app once, then install the production-signed APK.
Keep using the same release keystore for every future GitHub build.

## Play Protect

A proper release signature does not fully remove Play Protect’s “scan this
app” prompt for sideloaded APKs. That warning is mainly about install source
(not Play Store). Publishing on Google Play is what reduces it for end users.
