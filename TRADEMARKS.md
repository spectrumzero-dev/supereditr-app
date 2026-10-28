# SuperEditr name and logo

The code, including the icon files, is licensed under the [Apache License 2.0](LICENSE). The licence grants no rights to use the SuperEditr name or the pen-nib logo as a trademark (section 6), and this policy doesn't limit any right the licence gives you. SuperEditr and the pen-nib logo are trademarks of Spectrum Zero LLC, and they identify the official builds.

You may:

- build SuperEditr from source and use it yourself, with the name and icon unchanged;
- say that your project is "based on SuperEditr" or "a fork of SuperEditr";
- refer to SuperEditr by name when you write about it.

If you distribute your own build, whether free or paid:

- give it a different name and icon;
- don't suggest that it is official or endorsed.

Please also make these changes, so your build doesn't collide with the official app on the same Mac:

- change the bundle identifier `com.sghael.supereditr` (in `src-tauri/tauri.conf.json`) and the Keychain service in `src-tauri/src/secrets.rs`, so the two apps don't share macOS permissions and saved keys;
- if you turn on updates (`SUPEREDITR_RELEASE_BUILD`), point them at your own release feed and updater key.

Open an issue if you're unsure whether a use is fine.
