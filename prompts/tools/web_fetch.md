# WebFetch Tool

Fetch and extract content from web pages.

## Usage

- Fetches URL content and converts HTML to readable text
- Supports automatic retry on network failures
- Has a built-in cache to avoid repeated fetches
- Use for reading documentation, API references, etc.

## Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| url | string | Yes | The URL to fetch |
| prompt | string | No | A prompt to guide content extraction from the page |

## Best Practices

- Use for reading documentation and references
- Provide a prompt to focus extraction on relevant content
- Results are cached for 15 minutes
- Large pages may be truncated

## Limitations

- JavaScript-rendered content may not be available
- Some sites may block automated requests
- Binary files (images, PDFs) are not supported

## Examples

Fetch documentation:
```json
{"url": "https://docs.rs/tokio/latest/tokio/"}
```

Fetch with extraction prompt:
```json
{
  "url": "https://example.com/docs",
  "prompt": "Extract the API reference section about authentication"
}
```
