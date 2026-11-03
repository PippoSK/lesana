use crate::{
    adapters::api::shared::{
        app_state::AppState,
        error_presenter::{ApiError, ApiResult},
        session::session,
    },
    application::{
        contracts::AttachmentMetadata, usecases::attachments_usecase::MAX_ATTACHMENT_BYTES,
    },
};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use std::time::Duration;
use uuid::Uuid;

pub async fn list(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<AttachmentMetadata>>> {
    let owner = session(&app, &headers).await?;
    Ok(Json(app.services.list_attachments(owner, id).await?))
}

pub async fn upload(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
    request: Request,
) -> ApiResult<(StatusCode, Json<AttachmentMetadata>)> {
    let owner = session(&app, request.headers()).await?;
    app.services.ensure_owned(owner, id).await?;
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(malformed)?
        .to_owned();
    let boundary = multipart_boundary(&content_type)?;
    // The explicit body reader uses the upload limit; JSON routes retain 64 KiB.
    let bytes = tokio::time::timeout(
        Duration::from_secs(60),
        to_bytes(request.into_body(), MAX_ATTACHMENT_BYTES + 16 * 1024),
    )
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::REQUEST_TIMEOUT,
            "Nahrávanie súboru prekročilo časový limit.",
        )
    })?
    .map_err(|_| {
        ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Súbor môže mať najviac 10 MiB.",
        )
    })?;
    let file = parse_multipart(&boundary, &bytes)?;
    let metadata = app
        .services
        .upload_attachment(owner, id, &file.filename, file.bytes)
        .await?;
    Ok((StatusCode::CREATED, Json(metadata)))
}

pub async fn download(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path((id, attachment)): Path<(Uuid, Uuid)>,
) -> ApiResult<Response> {
    let owner = session(&app, &headers).await?;
    let file = app
        .services
        .download_attachment(owner, id, attachment)
        .await?;
    let mut response = Response::new(Body::from(file.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&file.metadata.content_type).map_err(|_| malformed())?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&download_disposition(&file.metadata.filename))
            .map_err(|_| malformed())?,
    );
    Ok(response)
}

pub async fn delete(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path((id, attachment)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let owner = session(&app, &headers).await?;
    app.services
        .delete_attachment(owner, id, attachment)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn malformed() -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        "Vyberte jeden súbor v poli file (multipart/form-data).",
    )
}

fn download_disposition(filename: &str) -> String {
    let encoded: String = filename
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_') {
                (*byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"document\"; filename*=UTF-8''{encoded}")
}

/// This endpoint accepts exactly one part, bounded headers and no preamble/epilogue.
/// Browser FormData uses this subset; reject extra parts before persisting anything.
struct MultipartFile {
    filename: String,
    bytes: Vec<u8>,
}

fn multipart_boundary(content_type: &str) -> ApiResult<String> {
    let (kind, parameters) = parameters(content_type)?;
    if !kind.eq_ignore_ascii_case("multipart/form-data") {
        return Err(malformed());
    }
    let boundaries: Vec<_> = parameters
        .into_iter()
        .filter(|(name, _)| name == "boundary")
        .collect();
    if boundaries.len() != 1 {
        return Err(malformed());
    }
    let boundary = boundaries.into_iter().next().unwrap().1;
    if boundary.is_empty()
        || boundary.len() > 70
        || !boundary
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"'()+_,-./:=?".contains(&b))
    {
        return Err(malformed());
    }
    Ok(boundary)
}

fn parse_multipart(boundary: &str, body: &[u8]) -> ApiResult<MultipartFile> {
    let opening = format!("--{boundary}\r\n");
    let body = body
        .strip_prefix(opening.as_bytes())
        .ok_or_else(malformed)?;
    let header_end = body
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .filter(|n| *n <= 8192)
        .ok_or_else(malformed)?;
    let headers = std::str::from_utf8(&body[..header_end]).map_err(|_| malformed())?;
    let mut disposition = None;
    for line in headers.split("\r\n") {
        let (name, value) = line.split_once(':').ok_or_else(malformed)?;
        if name.eq_ignore_ascii_case("content-disposition") {
            if disposition.replace(value.trim()).is_some() {
                return Err(malformed());
            }
        } else if !name.eq_ignore_ascii_case("content-type") {
            return Err(malformed());
        }
    }
    let (kind, attributes) = parameters(disposition.ok_or_else(malformed)?)?;
    if !kind.eq_ignore_ascii_case("form-data") {
        return Err(malformed());
    }
    let mut name = None;
    let mut filename = None;
    for (key, value) in attributes {
        match key.as_str() {
            "name" => {
                if name.replace(value).is_some() {
                    return Err(malformed());
                }
            }
            "filename" => {
                if filename.replace(value).is_some() {
                    return Err(malformed());
                }
            }
            _ => return Err(malformed()),
        }
    }
    if name.as_deref() != Some("file") {
        return Err(malformed());
    }
    let filename = filename.ok_or_else(malformed)?;
    let data = &body[header_end + 4..];
    let marker = format!("\r\n--{boundary}");
    let end = data
        .windows(marker.len())
        .enumerate()
        .find_map(|(index, bytes)| {
            if bytes == marker.as_bytes() {
                let tail = &data[index + marker.len()..];
                if tail.starts_with(b"--") || tail.starts_with(b"\r\n") {
                    return Some(index);
                }
            }
            None
        })
        .ok_or_else(malformed)?;
    let tail = &data[end + marker.len()..];
    if tail != b"--\r\n" && tail != b"--" {
        return Err(malformed());
    }
    if end > MAX_ATTACHMENT_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Súbor môže mať najviac 10 MiB.",
        ));
    }
    Ok(MultipartFile {
        filename,
        bytes: data[..end].to_vec(),
    })
}

fn parameters(value: &str) -> ApiResult<(String, Vec<(String, String)>)> {
    if value.len() > 8192 || value.chars().any(|c| c.is_control()) {
        return Err(malformed());
    }
    let (kind, mut rest) = value.split_once(';').unwrap_or((value, ""));
    let mut result = Vec::new();
    while !rest.trim().is_empty() {
        let (name, remaining) = rest
            .trim_start()
            .split_once('=')
            .ok_or_else(malformed)?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'*')
        {
            return Err(malformed());
        }
        let remaining = remaining.trim_start();
        let (value, suffix) = if let Some(quoted) = remaining.strip_prefix('"') {
            let mut decoded = String::new();
            let mut escaped = false;
            let mut closing = None;
            for (index, c) in quoted.char_indices() {
                if escaped {
                    decoded.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    closing = Some(index + 1);
                    break;
                } else {
                    decoded.push(c);
                }
            }
            (decoded, &quoted[closing.ok_or_else(malformed)?..])
        } else {
            let end = remaining.find(';').unwrap_or(remaining.len());
            (remaining[..end].trim().to_owned(), &remaining[end..])
        };
        result.push((name, value));
        rest = if suffix.trim().is_empty() {
            ""
        } else {
            suffix.trim_start().strip_prefix(';').ok_or_else(malformed)?
        };
    }
    Ok((kind.trim().to_owned(), result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(filename: &str, content: &[u8]) -> Vec<u8> {
        let mut body = format!("--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n").into_bytes();
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n--boundary--\r\n");
        body
    }

    #[test]
    fn multipart_preserves_binary_and_unicode_and_rejects_extra_parts() {
        let file = parse_multipart(
            "boundary",
            &body("Príloha; zmluva.txt", b"abc\r\n--boundaryX\0def"),
        )
        .unwrap_or_else(|_| panic!("valid file"));
        assert_eq!(file.filename, "Príloha; zmluva.txt");
        assert_eq!(file.bytes, b"abc\r\n--boundaryX\0def");
        let multiple = body("a.txt", b"a\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.txt\"\r\n\r\nb");
        assert!(parse_multipart("boundary", &multiple).is_err());
        assert!(parse_multipart("boundary", b"malformed").is_err());
        assert!(
            parse_multipart(
                "boundary",
                &body("big.txt", &vec![b'x'; MAX_ATTACHMENT_BYTES + 1]),
            )
            .is_err()
        );
    }

    #[test]
    fn header_boundaries_and_download_names_cannot_inject_headers() {
        assert_eq!(
            multipart_boundary("multipart/form-data; boundary=\"boundary\"")
                .unwrap_or_else(|_| panic!("valid boundary")),
            "boundary"
        );
        assert!(multipart_boundary("multipart/form-data; boundary=a; boundary=b").is_err());
        assert!(multipart_boundary("multipart/form-data; boundary=\"a\r\nb\"").is_err());
        assert!(parameters("form-data; name=\"unterminated").is_err());
        assert_eq!(
            download_disposition("Príloha\r\n.txt"),
            "attachment; filename=\"document\"; filename*=UTF-8''Pr%C3%ADloha%0D%0A.txt"
        );
    }
}
