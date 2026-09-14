# F-Droid Submission Guide: Torrent Player (`webtor.app`)

This document provides a comprehensive, step-by-step guide for submitting **Torrent Player** (`webtor.app`) to the official [F-Droid repository](https://gitlab.com/fdroid/fdroiddata).

> [!NOTE]
> **Active Submission**: Merge Request [**!48893**](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893) was submitted by `@karmugilrc` and is currently open on GitLab.

---

## 1. Overview of the F-Droid Inclusion Workflow

F-Droid is an installable catalog of FOSS (Free and Open Source Software) applications for the Android platform. Unlike standard app stores, F-Droid compiles applications directly from upstream source code repositories using its own automated build infrastructure.

The inclusion workflow follows these high-level stages:
1. **Metadata Recipe**: A build recipe (`metadata/webtor.app.yml`) is authored describing how F-Droid's builder clones the repository, checks out the release tag, sets up dependencies, and invokes Gradle.
2. **Merge Request (MR)**: The recipe is submitted via a Merge Request to [`fdroid/fdroiddata`](https://gitlab.com/fdroid/fdroiddata).
3. **Automated CI Validation**: F-Droid CI runs automated linters (`fdroid lint`, `fdroid checkupdates`) and verification pipelines.
4. **Review & Merge**: F-Droid maintainers review the recipe and verify adherence to the F-Droid Inclusion Policy.
5. **Scheduled Build Cycle**: Once merged, F-Droid's build servers compile the APK, sign it with the official F-Droid release key, and publish it to the F-Droid repository index.

---

## 2. Requirements Checklist

Before opening the Merge Request, ensure that all upstream prerequisites are satisfied:

| Requirement | Status | Details |
| :--- | :--- | :--- |
| **FOSS License** | Verified | MIT License (`LICENSE`) |
| **No Proprietary Dependencies** | Verified | Zero proprietary libraries, zero Google Play Services dependencies |
| **No Tracking / Ads / Analytics** | Verified | No Anti-Features required (`NoAdware`, `NoTracking`) |
| **Release Tag** | Verified | Tag `v1.4.2` matching `versionName = 1.4.2` and `versionCode = 19` |
| **Fastlane Metadata** | Verified | Located at `fastlane/metadata/android/en-US/` (title, descriptions, icon, screenshots, changelogs) |
| **Reproducible Build Script** | Verified | Native addons and Node runtime prebuild via `scripts/vendor-node.sh` and `scripts/build-native-addons.sh` |
| **Package ID** | Verified | `webtor.app` |

---

## 3. The F-Droid Build Recipe (`webtor.app.yml`)

The recipe has been generated at the repository root and is ready to be added to `fdroiddata`:

```yaml
Categories:
  - Internet
  - Multimedia
License: MIT
AuthorName: karmugilen
SourceCode: https://github.com/karmugilen/torrent-player
IssueTracker: https://github.com/karmugilen/torrent-player/issues
Changelog: https://github.com/karmugilen/torrent-player/releases

Summary: Stream and download torrents directly on Android with external media players
Description: |-
  Torrent Player is a lightweight, privacy-focused torrent client and streaming engine for Android.
  Features:
  - Single-contract Download to local storage (Downloads/Webtor).
  - Stream in-progress video files directly to VLC, Nova Video Player, MPV, or Just Player.
  - Bitfield piece visualizer and selective file downloading.
  - Low memory usage (96MB V8 heap cap) and battery-friendly adaptive polling.
  - Modern Material 3 UI with dark mode and animated preview cards.
  - 100% Free and Open Source with no tracking, analytics, or ads.

RepoType: git
Repo: https://github.com/karmugilen/torrent-player.git

Builds:
  - versionName: 1.4.2
    versionCode: 19
    commit: v1.4.2
    subdir: android
    gradle:
      - assembleRelease
    prebuild:
      - (cd ../engine && npm ci --omit=optional --omit=dev)
      - ../scripts/vendor-node.sh
      - ../scripts/build-native-addons.sh
    ndk: r26b

AutoUpdateMode: Version v%v
UpdateCheckMode: Tags
CurrentVersion: 1.4.2
CurrentVersionCode: 19
```

---

## 4. Step-by-Step GitLab Submission Instructions

### Step 1: Fork the `fdroiddata` Repository
1. Log into your account on [GitLab](https://gitlab.com).
2. Visit [https://gitlab.com/fdroid/fdroiddata](https://gitlab.com/fdroid/fdroiddata).
3. Click the **Fork** button (top right) to fork the project to your personal GitLab account/namespace.

---

### Step 2: Clone and Create a Working Branch

In your local terminal:

```bash
# Clone your fork
git clone https://gitlab.com/<YOUR_GITLAB_USERNAME>/fdroiddata.git
cd fdroiddata

# Create and switch to a new feature branch
git checkout -b add-webtor-app
```

*(Alternatively, you can perform these steps using the GitLab Web IDE or web interface directly on your fork).*

---

### Step 3: Copy Recipe into `metadata/`

Copy the recipe file `webtor.app.yml` from the `torrent-player` repo into the `metadata/` directory of `fdroiddata`:

```bash
# From within the cloned fdroiddata folder:
cp /path/to/torrent-player/webtor.app.yml metadata/webtor.app.yml
```

---

### Step 4: Commit and Push

```bash
git add metadata/webtor.app.yml
git commit -m "Add webtor.app"
git push origin add-webtor-app
```

---

### Step 5: Open Merge Request

1. Go to `https://gitlab.com/<YOUR_GITLAB_USERNAME>/fdroiddata`.
2. GitLab will present a banner prompting you to **Create merge request**.
3. Set the target repository to `fdroid/fdroiddata` branch `master`.
4. Fill in the MR details:
   - **Title**: `Add webtor.app (Torrent Player)`
   - **Description**: Use the template below.

#### MR Description Template

```markdown
### Application Details
- **Package Name**: `webtor.app`
- **Application Name**: Torrent Player
- **License**: MIT
- **Source Code**: https://github.com/karmugilen/torrent-player
- **Issue Tracker**: https://github.com/karmugilen/torrent-player/issues
- **Summary**: Stream and download torrents directly on Android with external media players

### Checklist
- [x] The upstream repository is public and licensed under a free software license.
- [x] The source code contains no non-free binary blobs or proprietary SDKs.
- [x] Does not contain proprietary tracking, analytics, or advertising libraries.
- [x] Application builds with standard Gradle and Android NDK r26b.
- [x] Fastlane metadata and screenshots are present in `fastlane/metadata/android/en-US/`.
- [x] Version matches release tag `v1.4.2` (`versionCode 19`).
- [x] I am the author / upstream maintainer of this project.
```

5. Click **Create merge request**.

---

## 5. What Happens Next: Review & Publication

### Automated CI Checks
Once the Merge Request is opened, the F-Droid CI runner will trigger several jobs:
1. **`lint` (`fdroid lint`)**: Checks YAML syntax, URL accessibility, licensing, and metadata compliance.
2. **`checkupdates` (`fdroid checkupdates`)**: Confirms that tags in the upstream git repository resolve cleanly to the version specified in the recipe.
3. **Build test (optional/scheduled)**: In some cases, maintainers will run a test build in the official F-Droid Debian build container to verify compilation.

### Review and Feedback
- Maintainers may leave comments or ask clarifying questions regarding build steps or dependencies.
- If any adjustments are requested, push additional commits to your `add-webtor-app` branch on GitLab; the MR will update automatically.

### Release & Publication Cycle
- Once approved, your MR is merged into `fdroid/fdroiddata:master`.
- F-Droid runs recurring build cycles (typically every few days).
- The F-Droid build engine will build the APK, sign it with F-Droid's key, and index it into the public F-Droid repository.
- Within 24–72 hours after merging, **Torrent Player** will become searchable and installable in the F-Droid client app!

---

## 6. Maintaining Future Releases

Because `AutoUpdateMode: Version v%v` and `UpdateCheckMode: Tags` are configured:
1. Whenever a new release is prepared in `torrent-player`, create and push a git tag matching `vX.Y.Z` (e.g. `git tag -a v1.4.3 -m "Release 1.4.3" && git push origin v1.4.3`).
2. Update the changelog under `fastlane/metadata/android/en-US/changelogs/<newVersionCode>.txt`.
3. F-Droid's automated bot `fdroid checkupdates` periodically scans for new release tags and automatically opens an update MR for your app without requiring manual recipe changes!
