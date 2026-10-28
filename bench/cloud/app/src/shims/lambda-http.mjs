// Function URL (payload 2.0) events to Request, Response to the URL's result.

export function toRequest(event) {
  const http = event.requestContext.http;
  const qs = event.rawQueryString ? `?${event.rawQueryString}` : "";
  const headers = new Headers();
  for (const [k, v] of Object.entries(event.headers ?? {})) headers.set(k, v);
  if (event.cookies?.length) headers.set("cookie", event.cookies.join("; "));
  const hasBody = event.body != null && !["GET", "HEAD"].includes(http.method);
  const body = hasBody ? (event.isBase64Encoded ? Buffer.from(event.body, "base64") : event.body) : undefined;
  return new Request(`https://${event.requestContext.domainName}${event.rawPath}${qs}`, { method: http.method, headers, body });
}

export async function fromResponse(res) {
  const headers = {};
  const cookies = [];
  res.headers.forEach((v, k) => {
    if (k === "set-cookie") cookies.push(v);
    else headers[k] = v;
  });
  return { statusCode: res.status, headers, cookies, body: await res.text(), isBase64Encoded: false };
}
