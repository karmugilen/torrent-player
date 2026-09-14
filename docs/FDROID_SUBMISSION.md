# F-Droid submission: Torrent Player (`webtor.app`)

The active submission is [fdroiddata merge request !48893](https://gitlab.com/fdroid/fdroiddata/-/merge_requests/48893),
from `karmugilrc/fdroiddata:add-webtor-app` to `fdroid/fdroiddata:master`.
Updating that branch updates the existing submission. Do not open a duplicate MR.

## Build recipe

The canonical recipe is [`webtor.app.yml`](../webtor.app.yml). Copy it to
`metadata/webtor.app.yml` in fdroiddata. Descriptions, screenshots, icon and
changelogs live in `fastlane/metadata/android/en-US/` in this upstream repository.

The Gradle subdirectory is `android/app`, where the APK output is written.
The recipe must reference the full commit SHA of the tested release. Version
1.4.2's previous recipe was not buildable from a clean checkout: it required an
untracked binary Node archive. Downloading that archive would also fail F-Droid's
[source dependency requirement](https://f-droid.org/docs/Inclusion_Policy/).

Version 1.4.3 prepares an entirely source-built native dependency chain:

- F-Droid's existing `NodejsMobile` srclib is pinned to
  `959b6e8637c86fb1f63f1aaf72adc86c7e8c335d` (v18.20.4).
- `vendor/libdatachannel` and its nested dependencies are pinned Git submodules.
- npm installs the lockfile with lifecycle scripts disabled. This avoids native
  package installers downloading host binaries or invoking unpinned build tools.
- Unused libsrtp and usrsctp fuzzing fixtures are removed before the source scan.
  JavaScript dependencies are scanned normally, without binary exemptions.
- Native compilation runs in the recipe's `build` phase, after the source scan.
  Node, uTP and WebRTC compile with NDK r26b for `arm64-v8a`.
- Gradle stages JavaScript, applies the Android loader patches, and builds an
  unsigned release APK. F-Droid handles signing after acceptance.
- Python 3 with setuptools supports the Node build on Debian trixie. Java uses
  the build environment rather than a developer-specific absolute path.

See the [README build instructions](../README.md#building-from-source) for local
builds. A first Node build takes substantially longer than an incremental APK build.

## Validation and review

Run in an fdroiddata checkout with fdroidserver and the required SDK installed:

```sh
fdroid rewritemeta webtor.app
fdroid lint webtor.app
fdroid checkupdates --auto webtor.app
fdroid build --latest webtor.app
```

Use the same buildserver image as fdroiddata CI:
`registry.gitlab.com/fdroid/fdroidserver:buildserver-trixie`.
Do not mark the MR's build/pipeline checkbox complete unless those checks pass.
A failed pipeline with zero jobs is not evidence of either build success or a
compiler error. Investigate runner availability separately.

The maintainer's earlier requests are retained: `New app: Torrent Player` title,
App Inclusion template, valid `Download` / `Online Media Player` categories,
full commit SHA, `gradle: yes`, and separate shell commands in the recipe.

## Submit updates

After testing and pushing the referenced upstream release commit, run:

```sh
./scripts/submit-to-fdroid.sh
```

The script updates only the existing submission branch, checks for concurrent
changes, and skips a commit if the remote recipe is already identical. It never
force-pushes or creates another MR.

F-Droid maintainers control review, merge, builds, signing and publication.
An open or merged MR does not mean the app is already available in the catalogue;
confirm the [app page](https://f-droid.org/packages/webtor.app/) before announcing
availability. Publication timing is not guaranteed.

## Future releases

Update the app version and changelog, test the source build, then push the
release commit and tag. `UpdateCheckMode: Tags` and `AutoUpdateMode: Version`
allow F-Droid to discover later tagged releases. Keep source revision pins and
build instructions consistent when native dependencies change.

## Validation on 2026-09-14

The official build job [16483867794](https://gitlab.com/fdroid/fdroiddata/-/jobs/16483867794)
for the old 1.4.2 recipe failed with `npm: command not found`; the other seven
checks passed and the APK check was skipped. The revised recipe installs Node,
npm and native build prerequisites explicitly.

A clean checkout of `bb6845006bf1b78b567fefe5f9208d51a19cd57f` was compiled
inside F-Droid's `buildserver-trixie` image, with NDK r26b and fdroidserver 2.4.2.
The source scan and complete native/Gradle compilation passed. The final F-Droid
output lookup initially failed because the old recipe used `subdir: android`.
The recipe now uses `android/app`; output discovery and F-Droid's APK identity,
version, release-mode and ABI checks passed against the generated APK. Its
binary scan also passed. This is not a claim that the updated official pipeline
has passed; that still requires a new pipeline for the updated MR commit.

The unsigned 1.4.3/build 20 APK is 20,117,712 bytes with SHA-256:
`475ff422490fceb25df83aacb3d70b6cca85eff19a3ba2683f99c41fb518fe6b`.
