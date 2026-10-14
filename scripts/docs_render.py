"""Shared static documentation rendering; no runtime or build dependencies."""

from html import escape
import json
from pathlib import Path
import re
from string import Template
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
LAYOUT = Template((ROOT / "docs/templates/layout.html").read_text())
BASE = "https://asc.example.com/v1/connections/conn_example"


def esc(value):
    return escape(str(value), quote=True)


def code(value):
    return "<code>" + esc(value) + "</code>"


def link(url, title, class_name="inline-link"):
    return f'<a class="{class_name}" href="{esc(url)}">{esc(title)}</a>'


def page_url(slug):
    return "/docs/reference/" + slug + "/"


def nav_link(url, label, current):
    selected = ' is-current" aria-current="page' if url == current else ""
    return f'<a class="nav-link{selected}" href="{esc(url)}">{esc(label)}</a>'


def sidebar(entries, current, topics=None, domains=None):
    start = "".join(nav_link(url, title, current) for url, title in [("/docs/", "Introduction"), ("/docs/quickstart/", "Quickstart"), ("/docs/conventions/", "API conventions")])
    # Conventions is written by the full builder; omit until it exists.
    if not (DIST / "docs/conventions/index.html").exists():
        start = "".join(nav_link(url, title, current) for url, title in [("/docs/", "Introduction"), ("/docs/quickstart/", "Quickstart")])
    content = '<section class="nav-group"><h2>Get started</h2><nav>' + start + "</nav></section>"
    content += '<section class="nav-group"><h2>Reference</h2><nav>' + nav_link("/docs/reference/", "All GET endpoints", current) + "</nav></section>"
    if topics and domains:
        for domain in domains:
            members = [t for t in topics if t["domain"] == domain["id"]]
            active = any(current == "/docs/reference/topics/" + t["slug"] + "/" or any(page_url(s) == current for s in t["endpoints"]) for t in members)
            count = sum(len(t["endpoints"]) for t in members)
            content += f'<details class="nav-group"{" open" if active else ""}><summary>{esc(domain["title"])}<span class="nav-count">{count}</span></summary><nav>'
            for topic in members:
                content += nav_link("/docs/reference/topics/" + topic["slug"] + "/", topic["title"], current)
                if any(page_url(s) == current for s in topic["endpoints"]):
                    entry = next(e for e in entries if e["url"] == current)
                    content += nav_link(entry["url"], "↳ " + entry["title"], current)
            content += "</nav></details>"
    else:
        content += '<section class="nav-group"><h2>Example pages</h2><nav>' + "".join(nav_link(e["url"], e["title"], current) for e in entries) + "</nav></section>"
    return content


def write_page(url, title, description, article, entries, toc, group="Reference", topics=None, domains=None):
    current_section = "intro" if url == "/docs/" else "quickstart" if url == "/docs/quickstart/" else "reference"
    selected = 'class="is-current" aria-current="page"'
    rendered = LAYOUT.substitute(
        title=esc(title), description=esc(description), article=article,
        intro_current=selected if current_section == "intro" else "",
        quickstart_current=selected if current_section == "quickstart" else "",
        reference_current=selected if current_section == "reference" else "",
        breadcrumb=esc(group), sidebar=sidebar(entries, url, topics, domains),
        toc="".join(link("#" + identifier, label, "") for identifier, label in toc),
    )
    target = DIST / url.lstrip("/") / "index.html"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(rendered)


def header(title, summary, group="Reference"):
    return f'<header class="page-header" id="overview"><p class="page-kicker">{esc(group)}</p><h1>{esc(title)}</h1><p class="page-lede">{esc(summary)}</p></header>'


def section(identifier, title, body):
    return f'<section class="document-section" id="{esc(identifier)}"><h2>{esc(title)}</h2>{body}</section>'


def endpoint_line(route, identifier="route-code", upstream=False):
    return f'<div class="endpoint-line{" upstream" if upstream else ""}"><span class="method">GET</span><code id="{esc(identifier)}">{esc(route)}</code><button type="button" data-copy-target="{esc(identifier)}" aria-label="Copy endpoint"><span class="copy-label">Copy</span></button></div>'


def highlight_json(value):
    text = json.dumps(value, indent=2, ensure_ascii=False)
    pattern = r'"(?:[^"\\]|\\.)*"(?=\s*:)|"(?:[^"\\]|\\.)*"|\b(?:true|false|null)\b|(?<![\w"])\b-?\d+(?:\.\d+)?\b'
    result = []
    last = 0
    for match in re.finditer(pattern, text):
        result.append(esc(text[last:match.start()]))
        token = match.group()
        kind = "key" if token.startswith('"') and text[match.end():].lstrip().startswith(":") else "string" if token.startswith('"') else "literal"
        result.append(f'<span class="syntax-{kind}">{esc(token)}</span>')
        last = match.end()
    result.append(esc(text[last:]))
    return "".join(result)


def code_block(text, identifier, label="JSON", json_value=False):
    content = highlight_json(text) if json_value else esc(text)
    return f'<div class="code-example"><div class="code-toolbar"><span class="code-label">{esc(label)}</span><button class="code-copy" type="button" data-copy-target="{identifier}" aria-label="Copy {esc(label)}"><span class="copy-label">Copy</span></button></div><pre id="{identifier}" tabindex="0"><code>{content}</code></pre></div>'


PATH_EXAMPLES = {"appId": "1234567890", "appInfoId": "app_info_123", "versionId": "version_123", "buildId": "build_123", "locale": "en-US", "categoryId": "UTILITIES", "territoryId": "USA"}


def concrete_route(route):
    return re.sub(r"\{(\w+)\}", lambda m: PATH_EXAMPLES.get(m[1], re.sub(r"(?<!^)(?=[A-Z])", "_", m[1].removesuffix("Id")).lower() + "_123"), route)


def requests(route, query=None):
    path = concrete_route(route)
    query = query or {}
    if query:
        path += "?" + urlencode(query, safe=",")
    curl = f'BASE=\'{BASE}\'\n\ncurl --fail-with-body "$BASE{path}" \\\n  -H "Authorization: Bearer $ASC_FACADE_KEY"'
    javascript = f'const base = "{BASE}";\nconst response = await fetch(`${{base}}{path}`, {{\n  headers: {{ Authorization: `Bearer ${{process.env.ASC_FACADE_KEY}}` }}\n}});\n\nconst body = await response.json();\nif (!response.ok) {{\n  throw new Error(body.error?.message ?? `HTTP ${{response.status}}`);\n}}\nconsole.log(body);'
    python = f'import json\nimport os\nfrom urllib.request import Request, urlopen\nfrom urllib.error import HTTPError\n\nbase = "{BASE}"\nrequest = Request(\n    f"{{base}}{path}",\n    headers={{"Authorization": f"Bearer {{os.environ[\'ASC_FACADE_KEY\']}}"}},\n)\ntry:\n    with urlopen(request) as response:\n        print(json.load(response))\nexcept HTTPError as error:\n    detail = json.load(error)\n    raise RuntimeError(detail["error"]["message"]) from error'
    labels = [("curl", "cURL", curl), ("javascript", "JavaScript", javascript), ("python", "Python", python)]
    tabs = '<div class="code-tabs" role="tablist" aria-label="Request language">'
    panels = ""
    for index, (identifier, label, text) in enumerate(labels):
        active = index == 0
        tabs += f'<button type="button" role="tab" id="tab-{identifier}" aria-controls="request-{identifier}" aria-selected="{str(active).lower()}" tabindex="{0 if active else -1}">{label}</button>'
        panels += f'<div class="code-panel" role="tabpanel" id="request-{identifier}" aria-labelledby="tab-{identifier}"{" hidden" if not active else ""}>{code_block(text, "request-" + identifier + "-code", label)}</div>'
    return tabs + "</div>" + panels


def table(rows, first_title="Name"):
    html = f'<div class="table-wrap" tabindex="0" role="region" aria-label="{esc(first_title)} definitions"><table class="param-table"><thead><tr><th scope="col">{esc(first_title)}</th><th scope="col">Type</th><th scope="col">Description</th></tr></thead><tbody>'
    for row in rows:
        name, type_name, description = row[:3]
        meta = row[3] if len(row) > 3 else ""
        html += f'<tr><th scope="row"><span class="parameter-name">{esc(name)}</span>{("<span class=parameter-meta>" + esc(meta) + "</span>") if meta else ""}</th><td>{code(type_name)}</td><td>{description}</td></tr>'
    return html + "</tbody></table></div>"


def resource_rows(entries):
    return '<div class="resource-list">' + "".join(f'<a class="resource-row" href="{esc(e["url"])}"><span class="resource-title">{esc(e["title"])}</span><code>GET {esc(e["route"])}</code></a>' for e in entries) + "</div>"
