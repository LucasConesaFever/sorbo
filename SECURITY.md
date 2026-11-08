# Security policy

sorbo runs with macOS Accessibility permission, which lets it see and move every window, and `./wm.sh install` creates a code-signing certificate in your login keychain. Please report anything that could let other software or a crafted config file abuse that.

## Reporting a vulnerability

Report it privately through GitHub: the repository's **Security** tab → **Report a vulnerability**. Please don't open a public issue for a security problem.

You'll get an answer within a week. sorbo is maintained by one person in their spare time, so fixes go into the latest version on `master`; there are no older release branches.

## Supported versions

Only the latest commit on `master`.
