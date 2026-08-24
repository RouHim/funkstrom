use warp::{Filter, Reply};

/// Serve the OpenAPI spec at /api-docs/openapi.yaml
pub fn openapi_spec() -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
    warp::path!("api-docs" / "openapi.yaml")
        .and(warp::get())
        .map(|| {
            let spec = include_str!("../openapi.yaml").to_owned();
            warp::reply::with_header(spec, "Content-Type", "application/x-yaml")
        })
}

/// Serve the vendored Swagger UI stylesheet
pub fn swagger_css() -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
    warp::path!("api-docs" / "swagger-ui.css")
        .and(warp::get())
        .map(|| {
            let css = include_str!("../swagger-ui.css").to_owned();
            warp::reply::with_header(css, "Content-Type", "text/css")
        })
}

/// Serve the vendored Swagger UI bundle script
pub fn swagger_bundle_js() -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
    warp::path!("api-docs" / "swagger-ui-bundle.js")
        .and(warp::get())
        .map(|| {
            let js = include_str!("../swagger-ui-bundle.js").to_owned();
            warp::reply::with_header(js, "Content-Type", "application/javascript")
        })
}

/// Serve the Swagger UI HTML page at /api-docs
pub fn swagger_ui() -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
    warp::path!("api-docs").and(warp::get()).map(|| {
        let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Funkstrom API Documentation</title>
    <link rel="stylesheet" type="text/css" href="/api-docs/swagger-ui.css" />
    <style>
        body {
            margin: 0;
            padding: 0;
        }
        .swagger-ui .info {
            margin: 20px 0;
        }
        .swagger-ui .info .title {
            font-size: 36px;
            color: #0891b2;
        }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="/api-docs/swagger-ui-bundle.js"></script>
    <script>
        window.onload = function() {
            const ui = SwaggerUIBundle({
                url: "/api-docs/openapi.yaml",
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [SwaggerUIBundle.presets.apis],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
                layout: "BaseLayout",
                defaultModelsExpandDepth: 1,
                defaultModelExpandDepth: 1,
                docExpansion: "list",
                filter: true,
                tryItOutEnabled: true
            });
            window.ui = ui;
        };
    </script>
</body>
</html>"#;
        warp::reply::html(html.to_owned())
    })
}
