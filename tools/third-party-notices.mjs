#!/usr/bin/env node
// Renders THIRD_PARTY_LICENSES.html for a shipped extension bundle, in the same
// layout as the other CanKing extensions (overview of licenses, then each distinct
// license text with the packages that use it).
//
//   node tools/third-party-notices.mjs --out <file.html> --product <name>
//        [--cargo <package> [--target <triple>] [--cargo-args "<extra cargo tree args>"]]
//        [--npm <third-party-npm.json>]
//
// --cargo lists the crates compiled into <package> (normal dependencies only, no
// proc-macros or build/dev dependencies, resolved for --target and the given features)
// and reads each crate's license files from the cargo registry.
// --npm takes the package list written by canking-gui/third-party-licenses.ts.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense)(\.|-|$)/i;

// Recognised license texts, checked in order: [id, display name, pattern].
const KNOWN_LICENSES = [
    ["Apache-2.0", "Apache License 2.0", /Apache License\s+Version 2\.0/i],
    ["OFL-1.1", "SIL Open Font License 1.1", /SIL OPEN FONT LICENSE/i],
    ["Unicode-3.0", "Unicode License v3", /UNICODE LICENSE V3/i],
    ["Unlicense", "The Unlicense", /free and unencumbered software released into the public domain/i],
    ["Zlib", "zlib License", /provided 'as-is', without any express or implied\s+warranty/i],
    ["BSD-3-Clause", "BSD 3-Clause \"New\" or \"Revised\" License", /Neither the name of[\s\S]{0,200}?nor the names of/i],
    ["BSD-2-Clause", "BSD 2-Clause \"Simplified\" License", /Redistribution and use in source and binary forms/i],
    ["ISC", "ISC License", /Permission to use, copy, modify,? and\/or distribute this software for any/i],
    ["MIT", "MIT License", /Permission is hereby granted, free of charge/i],
];

function parseArgs(argv) {
    const args = {};
    for (let i = 0; i < argv.length; i += 2) {
        if (!argv[i].startsWith("--") || argv[i + 1] === undefined) {
            throw new Error(`bad argument: ${argv[i]}`);
        }
        args[argv[i].slice(2)] = argv[i + 1];
    }
    if (!args.out || !args.product) throw new Error("--out and --product are required");
    return args;
}

function licenseTexts(dir) {
    return fs
        .readdirSync(dir)
        .filter((f) => LICENSE_FILE.test(f) && fs.statSync(path.join(dir, f)).isFile())
        .sort()
        .map((f) => ({ file: f, text: fs.readFileSync(path.join(dir, f), "utf8") }));
}

function cargoNotices(pkg, target, extraArgs) {
    const extra = extraArgs ? extraArgs.split(/\s+/).filter(Boolean) : [];
    const tree = execFileSync(
        "cargo",
        ["tree", "-q", "-p", pkg, "--target", target, "-e", "normal,no-proc-macro", "--prefix", "none", "-f", "{p}", ...extra],
        { encoding: "utf8" },
    );
    // "name v1.2.3", "name v1.2.3 (*)" for repeats, "name v1.0.0 (/path)" for workspace crates (ours, skipped).
    const wanted = new Set();
    for (const line of tree.split("\n")) {
        const m = line.trim().match(/^(\S+) v(\S+)(.*)$/);
        if (m && !m[3].includes("(/") && !m[3].match(/\([A-Za-z]:/)) wanted.add(`${m[1]}@${m[2]}`);
    }

    const metadata = JSON.parse(
        execFileSync("cargo", ["metadata", "-q", "--format-version", "1", "--filter-platform", target], {
            encoding: "utf8",
            maxBuffer: 64 * 1024 * 1024,
        }),
    );
    return metadata.packages
        .filter((p) => wanted.has(`${p.name}@${p.version}`))
        .map((p) => ({
            name: p.name,
            version: p.version,
            license: p.license ?? (p.license_file ? `see ${p.license_file}` : "UNKNOWN"),
            repository: p.repository ?? undefined,
            licenseTexts: licenseTexts(path.dirname(p.manifest_path)),
        }));
}

function identify(file, text) {
    if (/^notice/i.test(file)) return ["NOTICE", "NOTICE files"];
    const known = KNOWN_LICENSES.find(([, , re]) => re.test(text));
    return known ? [known[0], known[1]] : ["Other", "Other license notices"];
}

// Groups identical license texts; packages without a license file are grouped by their declared license.
function groupLicenses(packages) {
    const groups = new Map();
    const add = (key, id, name, text, pkg) => {
        if (!groups.has(key)) groups.set(key, { id, name, text, usedBy: [] });
        const usedBy = groups.get(key).usedBy;
        if (!usedBy.includes(pkg)) usedBy.push(pkg);
    };
    for (const pkg of packages) {
        for (const t of pkg.licenseTexts) {
            const text = t.text.replace(/\r\n/g, "\n").trim();
            const [id, name] = identify(t.file, text);
            add(`${id}\n${text}`, id, name, text, pkg);
        }
        if (pkg.licenseTexts.length === 0) {
            const text = `No license text is shipped with this package. Declared license: ${pkg.license}.`;
            const known = KNOWN_LICENSES.find(([id]) => id === pkg.license);
            const [id, name] = known ? [known[0], `${known[1]} (no license text shipped)`] : [pkg.license, pkg.license];
            add(`declared\n${pkg.license}`, id, name, text, pkg);
        }
    }
    return [...groups.values()].sort((a, b) => a.name.localeCompare(b.name) || b.usedBy.length - a.usedBy.length);
}

const escapeHtml = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function packageUrl(pkg) {
    const repo = (pkg.repository ?? "").replace(/^git\+/, "").replace(/\.git$/, "");
    if (/^https?:\/\//.test(repo)) return repo;
    if (/^git:\/\//.test(repo)) return repo.replace(/^git:/, "https:");
    if (/^github:/.test(repo)) return `https://github.com/${repo.slice(7)}`;
    if (/^[\w.-]+\/[\w.-]+$/.test(repo)) return `https://github.com/${repo}`;
    return undefined;
}

function render(product, packages) {
    const groups = groupLicenses(packages);

    // Overview: number of packages per license id, anchored to the first text with that id.
    const overview = new Map();
    const anchors = new Map();
    for (const g of groups) {
        const base = g.id.replace(/[^A-Za-z0-9.-]+/g, "-");
        const seen = anchors.get(base) ?? 0;
        g.anchor = seen === 0 ? base : `${base}-${seen}`;
        anchors.set(base, seen + 1);
        const known = KNOWN_LICENSES.find(([id]) => id === g.id);
        const o = overview.get(g.id) ?? { name: known ? known[1] : g.name, anchor: g.anchor, count: 0 };
        o.count += g.usedBy.length;
        overview.set(g.id, o);
    }

    const overviewItems = [...overview.values()]
        .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
        .map((o) => `            <li><a href="#${escapeHtml(o.anchor)}">${escapeHtml(o.name)}</a> (${o.count})</li>`);

    const licenseItems = groups.map((g) => {
        const usedBy = g.usedBy.map((p) => {
            const label = escapeHtml(`${p.name} ${p.version}`);
            const url = packageUrl(p);
            return url
                ? `                    <li><a href="${escapeHtml(url)}">${label}</a></li>`
                : `                    <li>${label}</li>`;
        });
        return [
            `            <li class="license">`,
            `                <h3 id="${escapeHtml(g.anchor)}">${escapeHtml(g.name)}</h3>`,
            `                <h4>Used by:</h4>`,
            `                <ul class="license-used-by">`,
            ...usedBy,
            `                </ul>`,
            `                <pre class="license-text">${escapeHtml(g.text)}</pre>`,
            `            </li>`,
        ].join("\n");
    });

    return `<html>

<head>
    <meta charset="utf-8">
    <title>Third Party Licenses - ${escapeHtml(product)}</title>
    <style>
        @media (prefers-color-scheme: dark) {
            body {
                background: #333;
                color: white;
            }
            a {
                color: skyblue;
            }
        }
        .container {
            font-family: sans-serif;
            max-width: 800px;
            margin: 0 auto;
        }
        .intro {
            text-align: center;
        }
        .licenses-list {
            list-style-type: none;
            margin: 0;
            padding: 0;
        }
        .license-used-by {
            margin-top: -10px;
        }
        .license-text {
            max-height: 200px;
            overflow-y: scroll;
            white-space: pre-wrap;
        }
    </style>
</head>

<body>
    <main class="container">
        <div class="intro">
            <h1>Third Party Licenses</h1>
            <p>This page lists the licenses of the projects used in ${escapeHtml(product)}.</p>
        </div>

        <h2>Overview of licenses:</h2>
        <ul class="licenses-overview">
${overviewItems.join("\n")}
        </ul>

        <h2>All license text:</h2>
        <ul class="licenses-list">
${licenseItems.join("\n")}
        </ul>
    </main>
</body>

</html>
`;
}

const args = parseArgs(process.argv.slice(2));
const counts = [];
let packages = [];

if (args.cargo) {
    const crates = cargoNotices(args.cargo, args.target ?? "wasm32-wasip2", args["cargo-args"]);
    counts.push(`${crates.length} rust crates`);
    packages = packages.concat(crates);
}
if (args.npm) {
    const npm = JSON.parse(fs.readFileSync(args.npm, "utf8"));
    counts.push(`${npm.length} npm packages`);
    packages = packages.concat(npm);
}
packages.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

fs.writeFileSync(args.out, render(args.product, packages));
console.log(`wrote ${args.out} (${counts.join(", ")})`);

const unknown = packages.filter((p) => p.license === "UNKNOWN").map((p) => `${p.name} ${p.version}`);
if (unknown.length > 0) {
    console.error(`error: no license declared for: ${unknown.join(", ")}`);
    process.exit(1);
}
