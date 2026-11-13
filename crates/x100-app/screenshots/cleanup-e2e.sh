#!/usr/bin/env bash
# Undoes what capture-e2e.sh made outside this machine, from the record it left in
# WORK_DIR/created.txt. It releases the Homebox project, which tears down its production and dev
# stacks, and deletes the API keys named x100 that "Use local" minted. Homebox has no command for
# either that x100's edit key may run (an edit key cannot delete keys), so this goes the way
# "Use local" does: one script run inside the homebox-admin container with Homebox's own code,
# which makes a short-lived owner key, calls Homebox's own API with it on the container's
# loopback, and deletes that key last. No key leaves the container. It does not delete the GitHub
# repository: it prints the command, to be run by the builder.
#
# Usage: cleanup-e2e.sh WORK_DIR/created.txt
# `X100_DOCKER` names another docker program, as for the daemon; the dry run's stand-in is one.
set -euo pipefail

record=${1:?the created.txt that capture-e2e.sh wrote}
REPOSITORY=$(sed -n 's/^REPOSITORY=//p' "$record")
HOMEBOX_PROJECT=$(sed -n 's/^HOMEBOX_PROJECT=//p' "$record")
[[ -n $REPOSITORY ]] || { echo "no REPOSITORY in $record" >&2; exit 2; }
docker=${X100_DOCKER:-docker}

# Every name and id is looked up in Homebox again, never trusted from the record: the project by
# its repository, the keys by their name.
cleanup='import asyncio, sys
import httpx
from app.db import SessionLocal, engine
from app.models import Identity
from app.routes.identities import _new_token
REPO = sys.argv[1]
async def main():
    tok, h, prefix = _new_token()
    async with SessionLocal() as s:
        s.add(Identity(kind="token", name="x100-cleanup", email=None, enabled=True, token_hash=h, token_prefix=prefix, role="owner"))
        await s.commit()
    await engine.dispose()
    async with httpx.AsyncClient(base_url="http://127.0.0.1:8000/api", headers={"Authorization": "Bearer " + tok}, timeout=300) as c:
        mine = None
        try:
            r = await c.get("/projects")
            r.raise_for_status()
            for p in r.json():
                if p["repo_full_name"].lower() == REPO.lower() and p["managed"]:
                    r = await c.post("/projects/%s/release" % p["id"])
                    print("released the Homebox project", p["name"], "of", REPO, "answer", r.status_code)
            r = await c.get("/identities")
            r.raise_for_status()
            for i in r.json():
                if i["kind"] == "token" and i["name"] == "x100":
                    r = await c.delete("/identities/%s" % i["id"])
                    print("deleted the API key x100", i["token_prefix"] + "...", "answer", r.status_code)
                if i["kind"] == "token" and i["name"] == "x100-cleanup" and i["token_prefix"] == prefix:
                    mine = i["id"]
        finally:
            if mine is not None:
                r = await c.delete("/identities/%s" % mine)
                print("deleted its own short-lived key, answer", r.status_code)
asyncio.run(main())'

echo "Releasing ${HOMEBOX_PROJECT:-the Homebox project} of $REPOSITORY and deleting the x100 keys in homebox-admin."
"$docker" exec -w /srv homebox-admin python -c "$cleanup" "$REPOSITORY"
echo
echo "The GitHub repository is left for the builder to delete:"
echo "  gh repo delete $REPOSITORY --yes"
echo "(deleting a repository needs gh's delete_repo scope: gh auth refresh -h github.com -s delete_repo)"
