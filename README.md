# kawa

kawa is an ffmpeg-based radio streamer.

## Compiling

Install the dependencies:

- ffmpeg
- rustc (stable)
- cargo (stable)

```
$ cargo build --release
# cargo install
```

## Usage

Start by copying example_config.toml to the location of your choice and reading
through it. Batteries are not included - kawa needs to be paired with your own
software to find songs to stream. You will have to provide an external API that
kawa can query for songs to play and notify as new songs being played.

## API

Kawa provides an HTTP API for management the queue. Kawa will play songs from
the queue until it is exhausted, then request new tracks from
`[queue].random_song_api`.

### GET /np

**Response**

```json
{ track blob }
```

### GET /listeners

**Response**

```json
[
    {
        "mount": "stream256.opus",
        "path": "/stream256.opus?user=minus",
        "headers": [
            {
                "name": "User-Agent",
                "value": "Music Player Daemon 0.20.9"
            },
            ...
        ]
    },
    ...
]
```

### GET /queue

**Response**

```json
[
    { track blob },
    ...
]
```

### POST /queue/head

Inserts a track at the top of the queue.

**Request**

```json
{ track blob }
```

Note: track blob is an arbitrary JSON blob that Kawa will hold on to for you. At
a minimum it must include "path", the path to the audio source on the
filesystem.

**Response**

```json
{
    "success": true,
    "reason": null
}
```

### POST /queue/tail

Inserts a track at the bottom of the queue. See `/queue/head`.

### DELETE /queue/head

Unqueues the track at the top of the queue.

**Response**

```json
{
    "success": true,
    "reason": null
}
```

### DELETE /queue/tail

Unqueues the track at the bottom of the queue. See `/queue/tail`.

### POST /queue/clear

Removes all tracks from the queue.

**Response**

```json
{
    "success": true,
    "reason": null
}
```

### POST /skip

Immediately skips to the next track in the queue.

**Response**

```json
{
    "success": true,
    "reason": null
}
```
