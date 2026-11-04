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

The server keeps one `claude` process started and waiting, so a request doesn't have to wait for Claude to start up. The waiting process uses the model and system prompt of the most recent request. Upon receiving a request with a different model for system prompt than the ones associated with the running process, the next waiting process will use that new model and/or system prompt. If no request arrives for `IDLE_TIMEOUT_SECONDS` (10 minutes by default), the server ends the waiting process.

`start` reads `PORT` and `IDLE_TIMEOUT_SECONDS` from your shell, so set them before running it.
