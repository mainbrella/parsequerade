"""List apps using an Apple team key from private environment configuration."""
import os
from pathlib import Path

from parsequerade import Client

client = Client(
    os.environ["ASC_ISSUER_ID"],
    os.environ["ASC_KEY_ID"],
    Path(os.environ["ASC_PRIVATE_KEY_PATH"]).read_text(),
)
query = {"limit": 50}
while True:
    page = client.apps(query)
    for app in page["items"]:
        print(app["id"], app.get("name") or "(unnamed)")
    if page["nextCursor"] is None:
        break
    query = {**query, "cursor": page["nextCursor"]}
