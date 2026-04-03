# project description

rest server for searching and adding ideas to a git repository

## security

all endpoints (REST and MCP) require OAuth 2.1 access tokens. no raw bearer token auth.

## data files

- each type has its own subdirectory: `IDEA/`, `TODO/`, `MEMORY/`, `NOTES/`
- files are named TYPE.DATE where TYPE can be TODO, IDEA, MEMORY, NOTES and DATE has the format YYYY-MM-DD-hh:mm and is the date of the first entry (e.g., `IDEA/IDEA.2026-04-01-14:30`)
- changes are written to the end of the file
- only UTF-8 characters allowed
- markdown is preferred for formatted text
- base64 encoded multimedia
- create a new file once the file is bigger than 100K

## entries

entries have the following format

```
date: YYYY-MM-DD-hh:mm
subject: single line subject
-----🔥😀🍕🔥
the text of the idea.
![smallimage](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGQAAABkAQMAAABKLAcXAAAABlBMVEX///+kpKSQYzjeAAAAlUlEQVR42u3SSwqEMAwG4BQXXXoErzGr6ZXmADLTo/UoOYJLF8VMYxIxuBYE/aGFL31BW3hyckan2lrAvTpTWDXtFU1+DD5OwMLnpq+fgEe9vNx7jk5vVSzcD6ohc99nUdIZop/7G1XOmaQ4i6TYSxG+skkRpcwzyA5CLi2qWFujWdURAlEBDbUsYElNuCkabOUtf8IfoPw1XKlKccwAAAAASUVORK5CYII=)
more text
-----🔥😀🍕🔥
```

- `subject` cannot have special control characters like tab, linefeed, carriage return, or any other non visible codes except for space (code 0x20) and cannot be longer than 120 bytes
- the begin and end markers start with `-----` and a sequence of 4 emojis that do not occur in the body of the text.
- the body cannot be bigger than 1M

## updates

to add an update

1. do a `git pull`
2. append the entry (using the current date) to the newest file unless the file is larger than 100K, otherwise start a new file with the entry
3. commit and push
4. if there is a conflict, discard changes and go back to step 1

## REST operations

- ADD operation has the following
    - `subject`
    - `text`
- SEARCH operation returns the newest entries matching the search stopping after 100K worth of entries has been returned. it has has the following
    - 'subject' - search for just the subject, can be missing
    - 'text' - search for subject and text, can be missing
    - 'after' - search for starting after the given date, can be missing
    - 'before' - search for starting before the given date, can be missing

## MCP interface

the server exposes an MCP (model context protocol) endpoint at `POST /mcp` with two tools:
- `add` - add a new entry (type, subject, text)
- `search` - search entries (subject, text, after, before, type — all optional)

all endpoints use OAuth 2.1 with PKCE for authentication. the OAuth flow:
1. client discovers metadata via `/.well-known/oauth-protected-resource` and `/.well-known/oauth-authorization-server`
2. client registers via `POST /oauth/register`
3. user authorizes via `/oauth/authorize` (enters the configured token as password)
4. client exchanges code for access token via `POST /oauth/token`

## configuration

config is read from `~/.config/gitideas.ini`:

```ini
port = 8080
token = your-oauth-password
repo = /path/to/git/repo
# optional:
# host = 127.0.0.1
# url = https://your-public-url.com
```

- `token` is the password users enter in the OAuth authorization page (not used as a direct bearer token)

## running the code

`gitideas`
