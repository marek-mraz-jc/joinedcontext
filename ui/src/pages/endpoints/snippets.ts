/** The same call as a program sends it; a closed endpoint names a token, never carries one. */
export function snippetsOf(url: string, open: boolean): { curl: string; python: string; javascript: string } {
  const quoted = JSON.stringify(url);
  return {
    curl: open
      ? `curl -H 'Accept: application/ld+json' '${url}'`
      : `curl -H 'Accept: application/ld+json' -H "Authorization: Bearer $TOKEN" '${url}'`,
    python: [
      open ? "import requests" : "import os, requests",
      "",
      `answer = requests.get(${quoted}, headers={`,
      `    "Accept": "application/ld+json",`,
      ...(open ? [] : [`    "Authorization": "Bearer " + os.environ["TOKEN"],`]),
      "}, timeout=30)",
      "answer.raise_for_status()",
      "print(answer.json())",
    ].join("\n"),
    javascript: [
      `const answer = await fetch(${quoted}, {`,
      `  headers: {`,
      `    Accept: "application/ld+json",`,
      ...(open ? [] : ["    Authorization: `Bearer ${process.env.TOKEN}`,"]),
      "  },",
      "});",
      "if (!answer.ok) throw new Error(`HTTP ${answer.status}`);",
      "console.log(await answer.json());",
    ].join("\n"),
  };
}
