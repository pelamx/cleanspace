# Mac App Store release

The App Store build is the normal app built with `--features appstore`. It differs from the website build in these ways:

- **Sandbox.** It runs in the App Sandbox. The first time you scan or map a folder, macOS asks you to pick that folder, and the app remembers your choice (a security-scoped bookmark). Settings live in `~/Library/Containers/com.pelamx.deskmonk`.
- **Payments.** Pro is sold as an Apple auto-renewable subscription through StoreKit 2 (`Store.swift`, linked by `build.rs`). This build has no Paddle checkout and no license keys.
- **No App Lock.** App Lock has to watch and close other apps, which the sandbox does not allow, so it is left out.
- **Trash.** Files go to the Trash through `NSFileManager` instead of AppleScript.

## Build

```sh
packaging/macos.sh appstore-test   # ad-hoc signed sandbox build to try locally: dist/appstore/out/deskMonk.app
packaging/macos.sh appstore        # signed upload package: dist/deskMonk-<version>-appstore.pkg
```

Both commands need the full Xcode, because only its Swift libraries include Intel. The upload build also needs these:

- "Apple Distribution" and "3rd Party Mac Developer Installer" certificates in the keychain (team LPP3L6Q6L8)
- the provisioning profile from step 1 below

Every upload gets a new build number automatically (`YYYYMMDD.HHMM`). Raise `version` in `Cargo.toml` for each new App Store version.

## One-time setup (developer.apple.com and App Store Connect)

1. **App ID and profile.** In Certificates, Identifiers & Profiles:
   - Create the App ID `com.pelamx.deskmonk` (explicit) with the In-App Purchase capability.
   - Create a **Mac App Store Connect** distribution provisioning profile for it.
   - Save the profile as `packaging/appstore/embedded.provisionprofile`. It is in .gitignore; don't commit it.
2. **App record.** In App Store Connect, go to Apps, press +, and choose New App:
   - Platform macOS, name "deskMonk", bundle ID `com.pelamx.deskmonk`, SKU e.g. `deskmonk`.
   - Category Utilities.
3. **Subscriptions.** Create a subscription group named "deskMonk Pro" with two products. The IDs must match `src/store.rs` exactly:
   - `com.pelamx.deskmonk.pro.monthly`, 1 month, $2.49 tier (website: $2.50)
   - `com.pelamx.deskmonk.pro.yearly`, 1 year, $24.99 tier (website: $25)

   Give each product a display name, a description and a review screenshot (a screenshot of the Pro dialog works). The app shows Apple's localized prices, so other currencies take care of themselves.
4. **Paid Apps Agreement.** Sign it under Business (bank and tax details). Subscriptions can't be sold or tested until it is active.
5. **App Privacy.** Answer to match `PrivacyInfo.xcprivacy` and `docs/privacy.html`:
   - Data used to track you: **No**
   - Collected, not linked to identity, purpose Analytics, only after opt-in:
     - Usage Data → Product Interaction
     - Diagnostics → Other Diagnostic Data (hardware model)
     - Identifiers → Device ID (the random install ID)
     - Location → Coarse Location (country, worked out by the server from the IP address)
   - Privacy policy URL: `https://deskmonk.app/privacy`. It is `docs/privacy.html`; `npx wrangler deploy` in `worker/` publishes `docs/` to deskmonk.app.
6. **Export compliance.** The Vault encrypts files (ChaCha20-Poly1305 and Argon2id, standard algorithms from open-source libraries). App Store Connect asks about this on the first upload. Your answer is a legal statement, so read Apple's "Complying with Encryption Export Regulations" and decide. Many apps that use only standard encryption qualify for an exemption, but some exemptions require a yearly self-classification report to the US BIS. Once you know the answer, you can add `ITSAppUsesNonExemptEncryption` to `packaging/Info.plist` so you aren't asked again on every build.

## Each release

1. Run `packaging/macos.sh appstore`.
2. Upload `dist/deskMonk-<version>-appstore.pkg` with **Transporter**. Transporter validates the package first, and its errors name what is missing.
3. In App Store Connect, open the new version and fill in:
   - **Screenshots:** at least one, 16:10, at 1280×800, 1440×900, 2560×1600 or 2880×1800. Take window shots with `screencapture -o -l <window id>`, then resize to one of these sizes with `sips`.
   - **Description, keywords, support URL** (`https://deskmonk.app`).
   - **Subscriptions:** add both products to the version for their first review.
   - **Review notes.** You can paste the text below.

     > deskMonk is a free disk cleaner with daily limits (2 GB of cleaning and 3 Vault encryptions of up to 200 MB per day). The "Pro" subscription (monthly or yearly, in-app purchase) removes the limits. Restore purchases and the subscription terms are in the Pro dialog (star button, top right). Because of the App Sandbox, the first scan asks the user to choose the folder to scan (Home is suggested). Files are moved to the Trash, except cache and log files in the Junk category, which are deleted. No account or login is needed.

4. Submit for review.

## Test before you submit

- **Sandbox flow:** run `packaging/macos.sh appstore-test` and open `dist/appstore/out/deskMonk.app`. Then:
  - Scan Home: the folder panel appears once, and later starts don't ask again.
  - Clean: files appear in the Trash.
  - Vault: encrypt a file, then open it.
- **Purchases:** create a Sandbox Apple Account (App Store Connect → Users and Access → Sandbox), and sign in to it under System Settings → App Store. Then buy, cancel and use Restore. StoreKit returns no products until the subscriptions exist in App Store Connect and the Paid Apps Agreement is active. Until then, the Pro dialog shows the fixed prices, and buying fails with an error.
