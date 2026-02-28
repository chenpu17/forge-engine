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
| selector | string | No | CSS selector to extract specific content |

## Best Practices

- Use for reading documentation and references
- Provide CSS selectors to extract relevant content
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

Extract specific section:
```json
{
  "url": "https://example.com/docs",
  "selector": "article.main-content"
}
```
