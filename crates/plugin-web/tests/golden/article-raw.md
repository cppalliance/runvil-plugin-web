url: http://localhost:PORT/guide/install/article
truncated: false
extraction: raw-html

Installing the Toolchain

[Home](http://localhost:PORT/) [Docs](http://localhost:PORT/docs/) [Blog](http://localhost:PORT/blog/)

# Installing the Toolchain

This guide walks through installing the toolchain on a fresh machine, from the package manager to the first build. Each step names the command to run and what it should print, so a reader can tell at once whether it worked.

## Before you start

Read the [requirements page](http://localhost:PORT/docs/requirements.html) first, and keep the [release notes](https://example.org/release-notes) open in another tab. The [first build](#first-build) section below assumes both are done.

![A terminal after a successful install](http://localhost:PORT/guide/install/images/terminal.png)

![Where the toolchain installs its files](data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7)

![The release channels](http://localhost:PORT/guide/install/images/channels-small.png)

## Install the packages

Run these two commands. The first refreshes the package index, and the second installs `curl`, which the installer needs:

```sh
sudo apt update
sudo apt install curl
```

Then run the installer:

```
curl --proto '=https' -sSf https://sh.example.org | sh
```

### What the installer changes

*   It adds the toolchain to `~/.local/bin`.
*   It writes a profile script, which:
    1.  puts that directory on `PATH`, and
    2.  sets the default channel to *stable*.
*   It changes nothing outside your home directory.

## First build

Build the example project and compare the timings with this table:

| Machine | Cold build | Warm build |
| ------- | ---------- | ---------- |
| Laptop  | 42 s       | 3 s        |
| Desktop | 18 s       | 1 s        |

> A warm build reuses the artifacts of the last one, so it only compiles what changed.

If a step fails, the [troubleshooting guide](http://localhost:PORT/guide/troubleshooting/) lists the messages people see most often and what each one means.

Copyright Example Project. [Legal](http://localhost:PORT/legal)