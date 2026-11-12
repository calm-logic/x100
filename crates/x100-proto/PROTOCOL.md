# x100 client protocol

How a client (the desktop app, the terminal interface, a command) talks to the daemon. The types are in
this crate; this document is the reference for the wire. A test (`tests/protocol/document.rs`) fails if a
method or notification exists here and not in the types or the other way round, if the errors listed for a
method differ from the types, or if an example here is not accepted by the type it is an example of.

## Transport and framing

- The transport is a Unix domain socket. Its path is the daemon's business and is decided with the daemon
  (M5). The same messages go over a network transport later.
- The wire is JSON-RPC 2.0, **one JSON object per line**: UTF-8, ended by `\n`, with no newline inside it
  (JSON escapes the newlines in strings). JSON-RPC batches (arrays) are not used and are refused with
  `invalid_request`.
- A request has `id` and `method`. A response has the same `id` and either `result` or `error`, never both.
  A notification has `method` and no `id` and is never answered. A `result` is never `null`; a method that
  only does something returns `{}`. An `id` is a number or a string and is the client's to choose.
- A line is at most 64 MiB. The daemon refuses a larger one with `too_large` and closes the connection,
  since it cannot find the next line. A file's bytes are base64 in one request, so this is also the largest
  upload; the daemon may set a lower limit and answer `too_large`.
- A line that is not JSON gets `parse_error` with a `null` id. A request that cannot be read gets an error
  carrying its `id` whenever the id could be read.
- Requests on one connection may be sent without waiting. Responses may arrive in any order, and notifications
  are interleaved with them. The `id` pairs a response with its request.

## Values

- Ids are a kind prefix and a ULID: `prj_01JC6Q4ZB0Q1V7XK4N5W1D8M3E`. An id of the wrong kind for its field is
  `invalid_params`.
- A time is milliseconds since the Unix epoch, as an integer. Money is an integer count of millionths of a US
  dollar. A duration is `{"secs": n, "nanos": n}`.
- Every record and event is the type of `x100-core` serialised as it is there, and the examples below are
  produced from the types. Enums are snake_case: a unit variant is a string, a variant with a value is an
  object with one key.
- Bytes are standard base64 text.
- A credential's value appears in exactly three requests: `credentials.add` and `credentials.replace`
  (`value`). No response, event or notification has a field that can hold one, and the type that carries it
  in cannot be serialised by accident (see `secret.rs`).

## Connecting

The first request is `hello`. The client sends the highest protocol version it speaks and the daemon answers
with the version both use (this document describes version 1). A daemon that cannot speak any version at or
below the client's answers `unsupported_protocol`.

## Following the logs

There is one log for the daemon and one for each project. Each counts its events from 1 on its own: a
sequence number is only meaningful with the log it belongs to, and nothing is promised about the order in
which events of different logs arrive.

One subscription, `events.subscribe`, follows all of them.

- **Without `resume`**, the result carries a **snapshot**: the `DaemonState` and the `ProjectState` of every
  project, each as of a sequence number (`seq()` on the state, or `Snapshot.cursor()`). The subscription is
  registered in the same step as the snapshot is taken, so the notifications that follow are exactly the
  events after it: no gap and no repeat, in order within each log, however many events are being appended
  while the client connects.
- **With `resume`**, the client says how far it had read each log. The daemon notifies every later event of
  each log first, then new ones as they come, and the result carries no snapshot. A log the cursor lacks is
  read from its first event. A cursor ahead of a log is `rejected`.
- **Order of result and notices.** The result of `events.subscribe` always arrives before the first
  `events.event` of that subscription, even when the daemon has events ready to send. The same holds for
  `transcript.subscribe` and `transcript.entry`. A client may therefore learn the subscription id from the
  result and treat any notice carrying it as belonging to a subscription it knows.
- **One events subscription per connection.** A connection has at most one live `events.subscribe`. A second
  one is `rejected` until the first is ended with `unsubscribe` or by `subscription.closed`. Transcript
  subscriptions are not limited this way: a connection may follow several agents.
- A project created after the subscription is followed from its first event (`Created`), so a client that
  gets an event for a project it has no state for starts that project's state empty and applies it.
- Each notification is `events.event`, naming the log. The client applies it to the state with the same
  functions the daemon uses (`ProjectState::apply`, `DaemonState::apply`), and a fold error means the client
  missed or repeated an event and must subscribe again.
- A client that reads too slowly is cut off rather than allowed to grow the daemon's memory. It gets
  `subscription.closed` with `lagged` and subscribes again with a cursor.

The Log tab is a project's part of the stream, rendered. It opens at the newest events with `events.read`
(`newest_first`, `before` the last one it has) and then follows the subscription.

An agent's **transcript** is too large for the event log and has a subscription of its own,
`transcript.subscribe`, opened when the agent's view is. The entries after `after` come first, then new ones
as they are written, with none missed or repeated. Positions count from 1 per agent. `unsubscribe` ends
either kind of subscription, and closing the connection ends all of them.

An entry has `at`, the `driver` it came from, and an `item` whose `type` is one of `turn_started`, `text`,
`tool_call`, `tool_output`, `tool_result`, `usage`, `turn_ended` and `driver_changed`. A `tool_output` is a
piece of a running tool's terminal output, `chunk`, paired by `call` with its `tool_call`. It is how the
Team tab shows a tool live; the `tool_result` that follows carries the whole output. The types are
`x100_core::transcript::Entry` and `Item`.

## Worked example

The client says hello. The daemon answers with the version both will use.

```json client
{"jsonrpc":"2.0","id":1,"method":"hello","params":{"client":"x100-app","protocol":1}}
```

```json daemon
{"jsonrpc":"2.0","id":1,"result":{"daemon":"0.1.0","protocol":1}}
```

The client subscribes with no cursor. The result carries the snapshot: the daemon's state and each project's, here a project at its first event.

```json client
{"jsonrpc":"2.0","id":2,"method":"events.subscribe","params":{"resume":null}}
```

```json daemon
{"jsonrpc":"2.0","id":2,"result":{"snapshot":{"daemon":{"cool_offs":[],"credentials":{},"harnesses":{},"jobs":{},"machines":{},"projects":{},"seq":null,"settings":{"cool_off":{"nanos":0,"secs":600},"default_wait_deadline":{"nanos":0,"secs":86400},"last_turn_limit":{"nanos":0,"secs":180},"minimum_repeat":{"nanos":0,"secs":300},"retire_after_age":{"nanos":0,"secs":14400},"retire_after_tasks":10,"retire_at_context":70,"stall_timeout":{"nanos":0,"secs":300},"start_pack_share":50,"turns_without_progress":3}},"projects":{"prj_01M74XBRM00000000000000001":{"actions":{},"agents":{"agt_01M74XBRM00000000000000001":{"charter":"Own Fernleaf's launch, from brief to a live site.","drivers":[],"id":"agt_01M74XBRM00000000000000001","inbox":{"items":{}},"items":{},"joined":1794387601000,"memories":{},"open_tasks":[],"processes":{},"role":"CEO","skills":[],"spent":{"cost":0,"usage":{"cache_read":0,"cache_write_long":0,"cache_write_short":0,"input":0,"output":0}},"state":"planned","waits":{}}},"chat":[],"drivers":{},"files":{},"jobs":{},"messages":{},"project":{"ceo":"agt_01M74XBRM00000000000000001","created":1794387601000,"name":"Fernleaf","preview":null,"settings":{"allowed":[{"harness":"hns_00000000000000000000000002","model":"claude-opus"},{"harness":"hns_00000000000000000000000002","model":"claude-sonnet"},{"harness":"hns_00000000000000000000000003","model":"gpt-codex"}],"ask_me_about":"high_risk_only","budget":50000000,"lab_share":10},"team":{"ceo":"agt_01M74XBRM00000000000000001","members":{"agt_01M74XBRM00000000000000001":{"position":{"ceo":{"effort":{"max":"max","min":"low"}}},"retired":false}}}},"seq":1,"skills":[],"specs":{},"spend":{"lab":0,"leads":{},"project":0},"tasks":{},"trials":{},"usage":{"cache_read":0,"cache_write_long":0,"cache_write_short":0,"input":0,"output":0}}}},"subscription":1}}
```

Every event after the snapshot follows as a notification, in order, once. The next two events of the project's log are 2 and 3.

```json daemon
{"jsonrpc":"2.0","method":"events.event","params":{"envelope":{"at":1794387602000,"by":"builder","event":{"file_added":{"added_by":"builder","attached_to":"msg_01M74XBSK80000000000000002","hash":"sha256:sketch","id":"fil_01M74XBSK80000000000000003","media_type":"image/png","name":"sketch.png","size":48213}},"seq":2},"log":"project","project":"prj_01M74XBRM00000000000000001","subscription":1}}
```

```json daemon
{"jsonrpc":"2.0","method":"events.event","params":{"envelope":{"at":1794387603000,"by":"builder","event":{"message_sent":{"attachments":["fil_01M74XBSK80000000000000003"],"body":"A landing page for Fernleaf, a plant shop, with a waitlist. Sketch attached.","id":"msg_01M74XBSK80000000000000002","in_reply_to":null,"route":"builder_to_ceo"}},"seq":3},"log":"project","project":"prj_01M74XBRM00000000000000001","subscription":1}}
```

The builder opens an agent. The client follows its transcript from the start. The same connection carries both subscriptions.

```json client
{"jsonrpc":"2.0","id":3,"method":"transcript.subscribe","params":{"after":null,"agent":"agt_01M74XBRM00000000000000001","project":"prj_01M74XBRM00000000000000001"}}
```

```json daemon
{"jsonrpc":"2.0","id":3,"result":{"subscription":2}}
```

```json daemon
{"jsonrpc":"2.0","method":"transcript.entry","params":{"entry":{"at":1794387630000,"driver":"drv_01M74XBRM00000000000000004","item":{"call":"toolu_01","input":{"command":"cargo test"},"name":"bash","type":"tool_call"}},"position":42,"subscription":2}}
```

The builder denies an action that was already decided. The daemon answers with a typed error.

```json client
{"jsonrpc":"2.0","id":4,"method":"actions.decide","params":{"action":"act_01M74XBRM0000000000000000A","decision":"denied","project":"prj_01M74XBRM00000000000000001"}}
```

```json daemon
{"jsonrpc":"2.0","id":4,"error":{"code":-32002,"message":"action act_01KA... is not held"}}
```

The view closes, and the client ends the transcript subscription.

```json client
{"jsonrpc":"2.0","id":5,"method":"unsubscribe","params":{"subscription":2}}
```

```json daemon
{"jsonrpc":"2.0","id":5,"result":{}}
```


## Methods

### `hello`

The first request on a connection. `protocol` is the highest version the client speaks; the reply carries the version both sides use.

Params type `Hello`, result type `Welcome`.

Errors: unsupported_protocol

```json params
{
  "client": "x100-app",
  "protocol": 1
}
```

```json result
{
  "daemon": "0.1.0",
  "protocol": 1
}
```

### `events.subscribe`

Subscribes to the daemon's log and every project's. Without `resume` the result carries a snapshot, and every event notified on the subscription is after it, once, in order, per log. With `resume`, the events after the cursor are notified first and the result carries no snapshot. A project created later is followed from its first event.

Params type `Subscribe`, result type `Subscribed`.

Errors: rejected

```json params
{
  "resume": null
}
```

```json result
{
  "snapshot": {
    "daemon": {
      "cool_offs": [],
      "credentials": {},
      "harnesses": {},
      "jobs": {},
      "machines": {},
      "projects": {},
      "seq": null,
      "settings": {
        "cool_off": {
          "nanos": 0,
          "secs": 600
        },
        "default_wait_deadline": {
          "nanos": 0,
          "secs": 86400
        },
        "last_turn_limit": {
          "nanos": 0,
          "secs": 180
        },
        "minimum_repeat": {
          "nanos": 0,
          "secs": 300
        },
        "retire_after_age": {
          "nanos": 0,
          "secs": 14400
        },
        "retire_after_tasks": 10,
        "retire_at_context": 70,
        "stall_timeout": {
          "nanos": 0,
          "secs": 300
        },
        "start_pack_share": 50,
        "turns_without_progress": 3
      }
    },
    "projects": {
      "prj_01M74XBRM00000000000000001": {
        "actions": {},
        "agents": {
          "agt_01M74XBRM00000000000000001": {
            "charter": "Own Fernleaf's launch, from brief to a live site.",
            "drivers": [],
            "id": "agt_01M74XBRM00000000000000001",
            "inbox": {
              "items": {}
            },
            "items": {},
            "joined": 1794387601000,
            "memories": {},
            "open_tasks": [],
            "processes": {},
            "role": "CEO",
            "skills": [],
            "spent": {
              "cost": 0,
              "usage": {
                "cache_read": 0,
                "cache_write_long": 0,
                "cache_write_short": 0,
                "input": 0,
                "output": 0
              }
            },
            "state": "planned",
            "waits": {}
          }
        },
        "chat": [],
        "drivers": {},
        "files": {},
        "jobs": {},
        "messages": {},
        "project": {
          "ceo": "agt_01M74XBRM00000000000000001",
          "created": 1794387601000,
          "name": "Fernleaf",
          "preview": null,
          "settings": {
            "allowed": [
              {
                "harness": "hns_00000000000000000000000002",
                "model": "claude-opus"
              },
              {
                "harness": "hns_00000000000000000000000002",
                "model": "claude-sonnet"
              },
              {
                "harness": "hns_00000000000000000000000003",
                "model": "gpt-codex"
              }
            ],
            "ask_me_about": "high_risk_only",
            "budget": 50000000,
            "lab_share": 10
          },
          "team": {
            "ceo": "agt_01M74XBRM00000000000000001",
            "members": {
              "agt_01M74XBRM00000000000000001": {
                "position": {
                  "ceo": {
                    "effort": {
                      "max": "max",
                      "min": "low"
                    }
                  }
                },
                "retired": false
              }
            }
          }
        },
        "seq": 1,
        "skills": [],
        "specs": {},
        "spend": {
          "lab": 0,
          "leads": {},
          "project": 0
        },
        "tasks": {},
        "trials": {},
        "usage": {
          "cache_read": 0,
          "cache_write_long": 0,
          "cache_write_short": 0,
          "input": 0,
          "output": 0
        }
      }
    }
  },
  "subscription": 1
}
```

### `unsubscribe`

Ends a subscription, of events or of a transcript.

Params type `Unsubscribe`, result type `Done`.

Errors: unknown_subscription

```json params
{
  "subscription": 1
}
```

```json result
{}
```

### `events.read`

Reads a page of the daemon's log or a project's: newest first for the Log tab, or oldest first from a point.

Params type `ReadLog`, result type `LogPage`.

Errors: not_found

```json params
{
  "log": {
    "project": "prj_01M74XBRM00000000000000001"
  },
  "page": {
    "before": null,
    "limit": 2,
    "order": "newest_first"
  }
}
```

```json result
{
  "events": [
    {
      "at": 1794387601000,
      "by": "builder",
      "event": {
        "created": {
          "ceo": "agt_01M74XBRM00000000000000001",
          "charter": "Own Fernleaf's launch, from brief to a live site.",
          "effort": {
            "max": "max",
            "min": "low"
          },
          "name": "Fernleaf",
          "settings": {
            "allowed": [
              {
                "harness": "hns_00000000000000000000000002",
                "model": "claude-opus"
              },
              {
                "harness": "hns_00000000000000000000000002",
                "model": "claude-sonnet"
              },
              {
                "harness": "hns_00000000000000000000000003",
                "model": "gpt-codex"
              }
            ],
            "ask_me_about": "high_risk_only",
            "budget": 50000000,
            "lab_share": 10
          }
        }
      },
      "seq": 1
    },
    {
      "at": 1794387602000,
      "by": "builder",
      "event": {
        "file_added": {
          "added_by": "builder",
          "attached_to": "msg_01M74XBSK80000000000000002",
          "hash": "sha256:sketch",
          "id": "fil_01M74XBSK80000000000000003",
          "media_type": "image/png",
          "name": "sketch.png",
          "size": 48213
        }
      },
      "seq": 2
    }
  ],
  "log": "project"
}
```

### `transcript.read`

Reads a page of an agent's transcript, after a position.

Params type `ReadTranscript`, result type `TranscriptPage`.

Errors: not_found

```json params
{
  "after": 40,
  "agent": "agt_01M74XBRM00000000000000002",
  "limit": 100,
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "entries": [
    {
      "entry": {
        "at": 1794387630000,
        "driver": "drv_01M74XBRM00000000000000004",
        "item": {
          "text": "Reading the migration.",
          "type": "text"
        }
      },
      "position": 41
    }
  ]
}
```

### `transcript.subscribe`

Follows an agent's transcript: the entries after `after` are notified first, then new ones as they are written, with none missed or repeated.

Params type `SubscribeTranscript`, result type `TranscriptSubscribed`.

Errors: not_found

```json params
{
  "after": null,
  "agent": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "subscription": 2
}
```

### `daemon.get`

The daemon's state as a one-off read, for a command that does not subscribe.

Params type `GetDaemon`, result type `x100_core::fold::DaemonState`.

Errors: none

```json params
{}
```

```json result
{
  "cool_offs": [],
  "credentials": {},
  "harnesses": {},
  "jobs": {},
  "machines": {},
  "projects": {},
  "seq": null,
  "settings": {
    "cool_off": {
      "nanos": 0,
      "secs": 600
    },
    "default_wait_deadline": {
      "nanos": 0,
      "secs": 86400
    },
    "last_turn_limit": {
      "nanos": 0,
      "secs": 180
    },
    "minimum_repeat": {
      "nanos": 0,
      "secs": 300
    },
    "retire_after_age": {
      "nanos": 0,
      "secs": 14400
    },
    "retire_after_tasks": 10,
    "retire_at_context": 70,
    "stall_timeout": {
      "nanos": 0,
      "secs": 300
    },
    "start_pack_share": 50,
    "turns_without_progress": 3
  }
}
```

### `daemon.set_setting`

Changes one of the daemon's settings.

Params type `SetDaemonSetting`, result type `Done`.

Errors: rejected

```json params
{
  "setting": {
    "stall_timeout": {
      "nanos": 0,
      "secs": 600
    }
  }
}
```

```json result
{}
```

### `machines.set_cap`

Sets the most live drivers a machine may run.

Params type `SetMachineCap`, result type `Done`.

Errors: not_found

```json params
{
  "cap": 8,
  "machine": "mch_01M74XBRM0000000000000000C"
}
```

```json result
{}
```

### `harnesses.rescan`

Looks for harnesses on this machine again, and returns every one found.

Params type `RescanHarnesses`, result type `Rescanned`.

Errors: unavailable

```json params
{}
```

```json result
{
  "harnesses": [
    "hns_01M74XBRM00000000000000002",
    "hns_01M74XBRM0000000000000000D"
  ]
}
```

### `credentials.add`

Adds a credential. The value goes to the vault and is never returned by any method.

Params type `AddCredential`, result type `CredentialAdded`.

Errors: rejected

```json params
{
  "name": "Stripe test key",
  "scope": {
    "commands": [
      "stripe"
    ],
    "hosts": [
      "api.stripe.com"
    ],
    "projects": {
      "only": [
        "prj_01M74XBRM00000000000000001"
      ]
    }
  },
  "value": "sk_test_not_a_real_key"
}
```

```json result
{
  "credential": "ref_01M74XBRM00000000000000008"
}
```

### `credentials.replace`

Replaces a credential's value, keeping its reference and scope.

Params type `ReplaceCredential`, result type `Done`.

Errors: not_found

```json params
{
  "credential": "ref_01M74XBRM00000000000000008",
  "value": "sk_test_another_not_real"
}
```

```json result
{}
```

### `credentials.set_scope`

Sets the hosts, commands and projects a credential may be used for.

Params type `ScopeCredential`, result type `Done`.

Errors: not_found

```json params
{
  "credential": "ref_01M74XBRM00000000000000008",
  "scope": {
    "commands": [
      "stripe"
    ],
    "hosts": [
      "api.stripe.com"
    ],
    "projects": {
      "only": [
        "prj_01M74XBRM00000000000000001"
      ]
    }
  }
}
```

```json result
{}
```

### `credentials.remove`

Removes a credential and its value.

Params type `RemoveCredential`, result type `Done`.

Errors: not_found

```json params
{
  "credential": "ref_01M74XBRM00000000000000008"
}
```

```json result
{}
```

### `projects.list`

Lists projects with the sidebar's line for each.

Params type `ListProjects`, result type `ProjectList`.

Errors: none

```json params
{}
```

```json result
{
  "projects": [
    {
      "agents": 4,
      "live": true,
      "name": "Fernleaf",
      "path": "/home/ada/fernleaf",
      "project": "prj_01M74XBRM00000000000000001"
    }
  ]
}
```

### `projects.get`

One project's state as a one-off read.

Params type `GetProject`, result type `x100_core::fold::ProjectState`.

Errors: not_found

```json params
{
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "actions": {},
  "agents": {
    "agt_01M74XBRM00000000000000001": {
      "charter": "Own Fernleaf's launch, from brief to a live site.",
      "drivers": [],
      "id": "agt_01M74XBRM00000000000000001",
      "inbox": {
        "items": {}
      },
      "items": {},
      "joined": 1794387601000,
      "memories": {},
      "open_tasks": [],
      "processes": {},
      "role": "CEO",
      "skills": [],
      "spent": {
        "cost": 0,
        "usage": {
          "cache_read": 0,
          "cache_write_long": 0,
          "cache_write_short": 0,
          "input": 0,
          "output": 0
        }
      },
      "state": "planned",
      "waits": {}
    }
  },
  "chat": [],
  "drivers": {},
  "files": {},
  "jobs": {},
  "messages": {},
  "project": {
    "ceo": "agt_01M74XBRM00000000000000001",
    "created": 1794387601000,
    "name": "Fernleaf",
    "preview": null,
    "settings": {
      "allowed": [
        {
          "harness": "hns_00000000000000000000000002",
          "model": "claude-opus"
        },
        {
          "harness": "hns_00000000000000000000000002",
          "model": "claude-sonnet"
        },
        {
          "harness": "hns_00000000000000000000000003",
          "model": "gpt-codex"
        }
      ],
      "ask_me_about": "high_risk_only",
      "budget": 50000000,
      "lab_share": 10
    },
    "team": {
      "ceo": "agt_01M74XBRM00000000000000001",
      "members": {
        "agt_01M74XBRM00000000000000001": {
          "position": {
            "ceo": {
              "effort": {
                "max": "max",
                "min": "low"
              }
            }
          },
          "retired": false
        }
      }
    }
  },
  "seq": 1,
  "skills": [],
  "specs": {},
  "spend": {
    "lab": 0,
    "leads": {},
    "project": 0
  },
  "tasks": {},
  "trials": {},
  "usage": {
    "cache_read": 0,
    "cache_write_long": 0,
    "cache_write_short": 0,
    "input": 0,
    "output": 0
  }
}
```

### `projects.create`

Creates a project in a directory, starting a git repository there if there is none, and with it the CEO. `settings` defaults to the daemon's.

Params type `CreateProject`, result type `ProjectCreated`.

Errors: rejected, unavailable

```json params
{
  "name": "Fernleaf",
  "path": "/home/ada/fernleaf",
  "settings": {
    "allowed": [
      {
        "harness": "hns_01M74XBRM00000000000000002",
        "model": "claude-sonnet"
      }
    ],
    "ask_me_about": "high_risk_only",
    "budget": 50000000,
    "lab_share": 10
  }
}
```

```json result
{
  "project": "prj_01M74XBRM00000000000000001"
}
```

### `projects.set`

Changes a project setting: Ask me about, the total budget, the lab's share, or whether a harness and model are allowed. Turning one off hands the work of drivers using it on.

Params type `ChangeProjectSetting`, result type `Done`.

Errors: not_found, rejected

```json params
{
  "change": {
    "budget": 80000000
  },
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `projects.reflect`

Reflect now: delegates a reflection task to the CEO.

Params type `Reflect`, result type `Reflecting`.

Errors: not_found, wrong_state, unavailable

```json params
{
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "task": "tsk_01M74XBRM00000000000000005"
}
```

### `chat.brief`

Sends the brief, the project's first message, with files already uploaded attached. It is delegated to the CEO as a task.

Params type `SendBrief`, result type `BriefSent`.

Errors: not_found, wrong_state

```json params
{
  "attachments": [
    "fil_01M74XBRM00000000000000003"
  ],
  "body": "A landing page for a plant shop, with a waitlist.",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "message": "msg_01M74XBRM00000000000000006",
  "task": "tsk_01M74XBRM00000000000000005"
}
```

### `chat.send`

Sends a message to the CEO. The builder talks to no one else.

Params type `SendChat`, result type `ChatSent`.

Errors: not_found

```json params
{
  "attachments": [],
  "body": "Use the green from the sketch.",
  "in_reply_to": "msg_01M74XBRM00000000000000006",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "message": "msg_01M74XBRM00000000000000006"
}
```

### `tasks.raise`

Raises a task as a new idea.

Params type `RaiseTask`, result type `TaskRaised`.

Errors: not_found

```json params
{
  "ask": "The footer links 404 on mobile.",
  "attachments": [
    "fil_01M74XBRM00000000000000003"
  ],
  "part": "Website",
  "project": "prj_01M74XBRM00000000000000001",
  "title": "Footer links are broken"
}
```

```json result
{
  "task": "tsk_01M74XBRM00000000000000005"
}
```

### `files.upload`

Uploads a file's bytes (base64 in `data`) and records it. Returns the file with its hash, to attach to a message or a task.

Params type `UploadFile`, result type `FileUploaded`.

Errors: not_found, too_large

```json params
{
  "data": "iVBORyBub3QgcmVhbGx5",
  "media_type": "image/png",
  "name": "sketch.png",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "file": {
    "added_by": "builder",
    "attached_to": null,
    "hash": "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    "id": "fil_01M74XBRM00000000000000003",
    "media_type": "image/png",
    "name": "sketch.png",
    "size": 48213
  }
}
```

### `files.read`

Reads a file's bytes.

Params type `ReadFile`, result type `FileContent`.

Errors: not_found

```json params
{
  "file": "fil_01M74XBRM00000000000000003",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "data": "iVBORyBub3QgcmVhbGx5",
  "file": {
    "added_by": "builder",
    "attached_to": null,
    "hash": "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    "id": "fil_01M74XBRM00000000000000003",
    "media_type": "image/png",
    "name": "sketch.png",
    "size": 48213
  }
}
```

### `files.remove`

Removes a file to save space. The removal is logged.

Params type `RemoveFile`, result type `Done`.

Errors: not_found, wrong_state

```json params
{
  "file": "fil_01M74XBRM00000000000000003",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `jobs.cancel`

Cancels a scheduled job. A job that has fired, or was cancelled, is `wrong_state`.

Params type `CancelJob`, result type `Done`.

Errors: not_found, wrong_state

```json params
{
  "job": "job_01M74XBRM00000000000000009",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `actions.decide`

Approves or denies a held action. An action that is not held is `wrong_state`.

Params type `DecideAction`, result type `Done`.

Errors: not_found, wrong_state

```json params
{
  "action": "act_01M74XBRM0000000000000000A",
  "decision": "approved",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `agents.set_budget`

Sets a lead's budget. All leads' budgets together may not pass the project's total.

Params type `SetLeadBudget`, result type `Done`.

Errors: not_found, rejected

```json params
{
  "budget": 15000000,
  "lead": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `agents.set_effort`

Sets an agent's effort range. A driver outside the new range is handed on.

Params type `SetEffortRange`, result type `Done`.

Errors: not_found, rejected

```json params
{
  "agent": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001",
  "range": {
    "max": "high",
    "min": "low"
  }
}
```

```json result
{}
```

### `agents.replace_driver`

Replaces the driver acting for an agent. The work hands off the way any handoff does.

Params type `ReplaceDriver`, result type `Done`.

Errors: not_found, wrong_state

```json params
{
  "agent": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `agents.inbox`

What has been delivered to an agent and not yet handled.

Params type `ReadInbox`, result type `Inbox`.

Errors: not_found

```json params
{
  "agent": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "items": [
    {
      "being_handled": false,
      "id": "inb_01M74XBRM0000000000000000E",
      "item": {
        "task": {
          "task": "tsk_01M74XBRM00000000000000005"
        }
      }
    }
  ]
}
```

### `agents.waits`

What an agent is waiting for, each with its deadline.

Params type `ReadWaits`, result type `Waits`.

Errors: not_found

```json params
{
  "agent": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "waits": [
    {
      "deadline": 1794474000000,
      "deadline_job": "job_01M74XBRM00000000000000009",
      "id": "wai_01M74XBRM0000000000000000B",
      "on": {
        "report": "tsk_01M74XBRM00000000000000005"
      }
    }
  ]
}
```

### `drivers.stop`

Stops a driver. Its work is handed to another.

Params type `StopDriver`, result type `Done`.

Errors: not_found, wrong_state

```json params
{
  "driver": "drv_01M74XBRM00000000000000004",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{}
```

### `drivers.start_pack`

The start pack a driver was given.

Params type `ReadStartPack`, result type `StartPack`.

Errors: not_found

```json params
{
  "driver": "drv_01M74XBRM00000000000000004",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "text": "# Charter\n..."
}
```

### `memories.list`

An agent's memory notes with their text.

Params type `ListMemories`, result type `Memories`.

Errors: not_found

```json params
{
  "agent": "agt_01M74XBRM00000000000000002",
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "notes": [
    {
      "body": "Deploys go through `make ship`.",
      "commit": "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
      "name": "deploys.md"
    }
  ]
}
```

### `skills.list`

Skills with their text: shared ones, or one scope's.

Params type `ListSkills`, result type `Skills`.

Errors: not_found

```json params
{
  "project": "prj_01M74XBRM00000000000000001",
  "scope": "shared"
}
```

```json result
{
  "skills": [
    {
      "body": "---\nname: deploy\nversion: 3\n---\n",
      "skill": {
        "name": "deploy",
        "scope": "shared"
      },
      "version": 3
    }
  ]
}
```

### `trials.list`

Reflection's trials with both arms' runs and scores.

Params type `ListTrials`, result type `Trials`.

Errors: not_found

```json params
{
  "project": "prj_01M74XBRM00000000000000001"
}
```

```json result
{
  "trials": []
}
```

## Notifications

### `events.event`

One event of the daemon's log or a project's, on an events subscription.

```json notice
{
  "envelope": {
    "at": 1794387603000,
    "by": "builder",
    "event": {
      "message_sent": {
        "attachments": [
          "fil_01M74XBSK80000000000000003"
        ],
        "body": "A landing page for Fernleaf, a plant shop, with a waitlist. Sketch attached.",
        "id": "msg_01M74XBSK80000000000000002",
        "in_reply_to": null,
        "route": "builder_to_ceo"
      }
    },
    "seq": 3
  },
  "log": "project",
  "project": "prj_01M74XBRM00000000000000001",
  "subscription": 1
}
```

### `transcript.entry`

One entry of an agent's transcript, on a transcript subscription.

```json notice
{
  "entry": {
    "at": 1794387630000,
    "driver": "drv_01M74XBRM00000000000000004",
    "item": {
      "call": "toolu_01",
      "input": {
        "command": "cargo test"
      },
      "name": "bash",
      "type": "tool_call"
    }
  },
  "position": 42,
  "subscription": 2
}
```

### `subscription.closed`

The daemon ended a subscription itself.

```json notice
{
  "because": "lagged",
  "subscription": 1
}
```

## Errors

An error is `{"code": n, "message": "..."}`. The `message` is for people. The codes are:

| Name | Code | Meaning |
|---|---|---|
| `parse_error` | -32700 | The line was not JSON. |
| `invalid_request` | -32600 | The JSON was not a message this protocol uses. |
| `method_not_found` | -32601 | No such method. |
| `invalid_params` | -32602 | The params do not fit the method, an id is of the wrong kind, or bytes are not base64. |
| `internal` | -32603 | A fault in the daemon, not in the request. |
| `not_found` | -32001 | A project, agent, task, file or other record the request names does not exist. |
| `wrong_state` | -32002 | The record exists but is in a state that does not allow this: deciding an action that is not held, cancelling a job that has fired. |
| `rejected` | -32003 | The request is well formed but breaks a rule: a lead's budgets would pass the project's total, a directory is not usable as a project. The message says which. |
| `too_large` | -32004 | A line or a file over the limit. |
| `unsupported_protocol` | -32005 | The client's protocol version is not one the daemon speaks. |
| `unknown_subscription` | -32006 | The subscription id is not open on this connection. |
| `unavailable` | -32007 | The daemon cannot do it now: it is shutting down, or nothing on this machine can serve the request. |

## Resources

Every resource of the design is reached in one of three ways: read from the snapshot and kept current by the
event stream, read or changed by a method, or both. A method only changes a resource by asking the daemon,
which appends events; the client sees the result in the stream, not in the response.

| Resource | Read | Changed by |
|---|---|---|
| `/machines/` | snapshot, `daemon.get` | `machines.set_cap` |
| `/harnesses/` | snapshot, `daemon.get` | `harnesses.rescan`, `projects.set` (allow or disallow a harness and model) |
| `/credentials/` | snapshot, `daemon.get` (reference, name, scope: never the value) | `credentials.add`, `.replace`, `.set_scope`, `.remove` |
| daemon settings | snapshot, `daemon.get` | `daemon.set_setting` |
| `/projects/` | snapshot, `projects.list`, `projects.get` | `projects.create`, `projects.set`, `projects.reflect` |
| `/events` | `events.subscribe`, `events.read` | not changed by clients |
| `/agents/` | snapshot (team, state, current task and driver) | `agents.set_budget`, `agents.set_effort`, `agents.replace_driver` |
| `/agents/{id}/inbox` | `agents.inbox` | by the daemon |
| `/agents/{id}/waits` | `agents.waits` | by the daemon |
| `/agents/{id}/transcript` | `transcript.read`, `transcript.subscribe` | by the daemon |
| `/drivers/` | snapshot, `drivers.start_pack` | `drivers.stop` |
| `/tasks/` | snapshot | `chat.brief`, `tasks.raise` |
| `/messages/` | snapshot (the chat) | `chat.brief`, `chat.send` |
| `/jobs/` | snapshot | `jobs.cancel` |
| `/actions/` | snapshot | `actions.decide` |
| `/specs/` | snapshot | by agents |
| `/files/` | snapshot, `files.read` | `files.upload`, `files.remove` |
| `/ledger/` | snapshot (spend per project, lead and agent, tokens); each charge is an event, read with `events.read` | by the daemon |
| `/memories/` | `memories.list` | by agents |
| `/skills/` | snapshot (loaded skills and committed versions), `skills.list` | by agents and trials |
| `/trials/` | snapshot, `trials.list` | by the lab |
