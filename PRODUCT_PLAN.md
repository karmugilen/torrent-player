# Webtor product plan

This is the agreed direction for the next Webtor version.

## Core decisions

- Use a dark, modern interface with a navy background, mint accent color, rounded cards, clear progress states, and comfortable touch targets.
- Show torrent metadata before downloading content. The user reviews the files and chooses what to download.
- Ask for a destination before content download begins. Remember the last chosen destination.
- Use `Downloads/Webtor/<torrent>` by default on supported Android versions. Also offer Android's folder picker for a user-selected directory.
- Keep downloaded files when a torrent is paused or stopped.
- Make Delete explicit and confirm whether the user wants to remove the torrent only or remove the downloaded files too.
- Continue using the installed VLC/mpv or another compatible external player for playback.
- Keep downloads running through a foreground service when the user leaves the app.

## Main screens

### Library

The home screen shows saved torrents as cards. Each card includes:

- title and media type;
- download progress and downloaded/total size;
- current speed, peer count, and remaining time;
- destination folder;
- pause/resume, play, share/open, and delete actions;
- an error state with a retry action when a torrent fails.

The floating Add button opens a sheet where the user can paste a magnet, enter a torrent URL, or open a `.torrent` file.

### Prepare download

After metadata is available, show the torrent name and its file list before selecting a destination.

- Video files are selected by default; the user can select or deselect individual files.
- Show each file's name and size.
- Show the available phone storage before starting.
- Let the user choose the default Downloads location or a folder through `ACTION_OPEN_DOCUMENT_TREE`.
- Do not create content files until the user taps Start download.
- Cancel returns to the Library without leaving a partial content download.

### Storage

Add a Storage section with:

- free phone storage;
- Webtor-managed storage usage;
- remembered destination folder;
- a list of saved downloads sorted by size or date;
- cleanup for old temporary/cache data;
- an explicit “clear all downloaded files” action with confirmation.

Downloads written through a selected folder must remain visible to the user's file manager. The app should persist the granted folder permission and explain when that permission has been revoked.

## Delete behavior

When deleting a library item, offer two choices:

1. Remove from Webtor and keep the files.
2. Remove from Webtor and delete the downloaded files.

Deleting files must only target files created by Webtor. A failed or revoked folder permission should leave the library metadata recoverable and explain how to retry deletion from the user's file manager.

## Download lifecycle

1. Add magnet, URL, or torrent file.
2. Fetch metadata only.
3. Select files and destination.
4. Create destination files and start downloading.
5. Pause preserves verified and partial data.
6. Resume continues from existing data after rescanning it.
7. Play streams the selected file through the local HTTP server.
8. Stop removes the active engine session while preserving files and library metadata.
9. Delete removes the library item and optionally removes its files.

## Suggested additional features

- Search and filter the library by title, active, paused, completed, or error state.
- Sort by recently added, recently played, size, or progress.
- Wi-Fi-only downloading and a mobile-data confirmation.
- Maximum download speed and upload limit controls.
- Queue downloads and limit the number of active torrents.
- Automatic pause when the battery is low or storage falls below a threshold.
- Per-torrent file priority, including “download for playback first”.
- Remember the last played file and playback position where the external player supports it.
- Share a torrent's magnet link or `.torrent` metadata.
- Export and import the Webtor library metadata for backup.
- A privacy page showing enabled discovery methods and tracker settings.
- A notification with pause/resume, progress, and stop actions.

## Android storage rules

- Prefer `MediaStore.Downloads` for the default Downloads destination on Android 10 and newer.
- Use the Storage Access Framework for a user-selected folder and persist its URI permission.
- Avoid broad storage permissions for normal operation.
- Treat a revoked folder permission, missing file, or insufficient space as a recoverable error.
- Keep torrent metadata and library records separate from downloaded media so cleanup cannot silently lose the user's history.

## Acceptance checklist

- A magnet can be added without downloading content before confirmation.
- The user can select a subset of files and a destination folder.
- Downloaded files appear in the chosen folder and remain after stopping the app.
- Pause actually stops peer transfers and resume continues them.
- Playback works while a file is still downloading when enough pieces are available.
- Delete offers keep-files and erase-files choices.
- Storage usage is visible and old app-owned data can be cleaned safely.
- The app handles missing peers, revoked folder access, no space, and missing external players with readable errors.
- A fresh install has no storage permission prompt for the normal flow.
