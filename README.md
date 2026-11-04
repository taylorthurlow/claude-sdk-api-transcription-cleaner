# claude-sdk-api-transcription-cleaner

A local server that accepts OpenAI-compatible chat completion requests and answers them with `claude -p`. It is meant for cleaning up dictated transcripts.

If the prompt wraps the dictation in `<transcript>` tags, the server checks that Claude's reply is still a cleaned-up copy of it. If the reply looks like something else, the server returns the transcript unchanged.

It runs on macOS and needs the `claude` command on your `PATH`.

## Install

```sh
./install.sh
```

This builds the server and installs it to `~/.local/bin`.

## Usage

```sh
claude-sdk-api-transcription-cleaner start   # run in the background, and at every login
claude-sdk-api-transcription-cleaner stop    # stop, and don't start at login
claude-sdk-api-transcription-cleaner status  # show whether it's running
claude-sdk-api-transcription-cleaner serve   # run in this terminal
```

Point your client at `http://127.0.0.1:8765/v1`. The models are `haiku`, `sonnet`, and `opus`.

Set `PORT` to listen on a different port.
