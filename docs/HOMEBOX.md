# Homebox, as x100 will integrate with it

Status: research for M12I, 2026-11-12. Read-only: every request to Homebox was an unauthenticated GET, and no
credential file was read. Each point is from Homebox's code, from the running instance, from notes, or
unknown. The current Homebox code is `~/code/homebox` (`calm-logic/homebox-cloud`), not `~/code/homebox-public`.

## Where it is

- The admin app and API are at `http://127.0.0.1:7765`, bound to loopback, with the API under `/api`.
  `/api/healthz` answers `{"ok":true}`. `/api/openapi.json` has the title "Homebox Admin API" and no response
  schemas, so x100 types responses by hand. `/api/version` says `v0.0.0-dev` here, so features are detected
  from the OpenAPI paths.
- A local Homebox is found without credentials: a container named `homebox-admin`, its published port, the
  health answer and the OpenAPI title.

## Authentication

- **API keys are the only way for a program.** `hbk_` keys, sent as `Authorization: Bearer`, with a role: read
  (GET only), edit (everything except identities) or owner. Minted on the Identities page or by
  `POST /api/identities` with a session or an owner key, shown once. They never expire. A key is box-wide: it
  cannot be limited to one project. `GET /api/auth/me` tests one. An edit key cannot revoke itself.
- **No OAuth server.** Homebox is only an OAuth client of GitHub and Google.
- **No designed local access.** Loopback is not trusted: `/api/projects` from 127.0.0.1 answers 401. What
  exists is Docker: a user in the `docker` group can run Homebox's own code inside its container, which is
  owner authority over the whole box and cannot be narrowed.

## "Use local", as x100 can offer it

The daemon runs one fixed command in the `homebox-admin` container, once, after the builder confirms, which
mints an edit key named for x100 through Homebox's own code. The key goes straight to the vault. Every call
after that is plain HTTP to loopback with that key, exactly as for a remote Homebox. Costs: the daemon needs
Docker access for that one command, it depends on Homebox's internals until Homebox offers a supported
`mint-key` command (about 50 lines there), and the key is box-wide, so x100 allows only the calls it needs and
sends deploys through the gate. Not run yet: it is inferred from code and notes.

For a Homebox elsewhere: a host address and an API key the builder creates there.

Later, in Homebox: a browser approval flow (loopback redirect with PKCE, about 500 lines) would make both ways
one click with nothing copied, and keys limited to one project would narrow what x100 holds.

## The calls x100 needs

All under `/api`. R needs a read key, E an edit key.

- `GET /projects` (R): every project with `id`, `name`, `repo_full_name`, `managed`, `auto_deploy`,
  `environments`. There is no search: find by name on the client.
- `GET /projects/{id}` (R): with services, environments, and each instance's `url` and `status`.
- `POST /integrations/{id}/sync` (E), then `POST /projects/{id}/add` (E): make a repository a managed project.
  Adding creates production and dev environments and queues a first deploy in both.
- `POST /projects/{id}/environments/{env}/deploy` (E): deploys the branch head. No commit can be named.
- `GET /projects/{id}/deployments/{dep}` (R): status, `commit_sha`, `error`, `log_steps`, `log_tail`. Polled.
  Running is success. Failed carries the error. Also canceled, superseded, staged and several pending states.
- `POST /projects/{id}/release` (E): tears a project's stacks down and unmanages it.

## How source gets there

Only from a hosted git provider over https: GitHub or GitLab. No local path, archive or image. A Homebox
project is one repository. So an x100 project, which is a local repository with no remote, must be pushed to
a repository that a Homebox GitHub integration covers before Homebox can deploy it. That push leaves the
machine.

A repository holding only `index.html` deploys as it is: Homebox serves it as a plain static site. That
support is live in the running container and uncommitted in Homebox's working tree, so a Homebox update would
drop it.

## What an integration must handle

- `auto_deploy` is on by default, so a push deploys by webhook. Calling deploy as well runs it twice. Rule:
  with it on, push and wait for the deployment with that commit. With it off, call deploy and check the commit.
- A container that crash-loops still shows a running deploy: check the instance's status.
- A deploy on this Homebox is public at `<name>.<primary domain>` and is copied to every cluster peer.
- There is no sandbox target: the local target is the production Docker host.
- The link stores the instance, the project's `repo_full_name` as its stable key, its id and name as last
  seen, and the environment. A renamed project changes address. A released one refuses deploys until re-linked.

## Found since, from Homebox's code and a logged-in session

- `GET /api/domains` (R) lists domains with `name` and `is_primary`. `GET /api/integrations` (R) lists the
  connected accounts with `provider` and `account_login`. Each project also carries `domain` and
  `integration.account_login`.
- The deploy and deployment-list routes take the environment's id, not its name.
- `POST /projects/{id}/add` answers with the deployments it queued, so the first deploy is watched and not
  repeated.
- Homebox probes only instances that have a URL. The others stay `unknown`, so only an instance with a URL
  can be judged as crash-looping.
- A project's `domain_mode` is `container` (`<name>.<domain>`, and `<name>--dev.<domain>` for dev) or `base`
  (the project owns the domain: production at the apex, dev at `dev.<domain>`).
- `POST /projects/{id}/environments/{env}/stop` pauses an environment until it is resumed. Pushes while paused
  are staged and not run.
- "Use local" has been run once by hand in a narrower form: a session signed inside the container, not a
  minted key. The x100 mint command itself is still unrun.

## Unknown

The cluster's peers, and whether releasing a project also removes it from peers.
