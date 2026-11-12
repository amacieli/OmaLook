/*
 * omalook-render: renders one HTML mail body with WebKitGTK to a PNG.
 *
 * Why a separate process: a GTK widget cannot be embedded in the Qt/Quickshell window on Wayland,
 * and WebKit must never be able to take the UI down. The UI runs this per message, kills it if the
 * user moves on, and shows the PNG. A crash, hang or timeout here only means "fall back".
 *
 *   omalook-render --width CSS_PX [--scale N] [--images] --out FILE.png  < mail.html
 *
 * stdout, one JSON line:
 *   {"width":W,"height":H,"scale":S,"bg":"#rrggbb","links":[{"x":..,"y":..,"w":..,"h":..,"href":".."}]}
 * (link boxes in CSS px; multiply by scale for image pixels). Exit codes: 0 ok, 2 no display /
 * bad usage, 3 timeout, 4 render failed.
 *
 * Isolation: ephemeral data manager (no cookies, cache or storage), page scripts off (JavaScript markup
 * disabled; a content-security-policy as well), no navigation away from our own about:blank document and
 * no popups, and a content blocker: loopback/private addresses are never fetched, and nothing remote at all
 * unless --images is given (SEC-008/SEC-009 in the 2026-11-12 review).
 */
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
#include <glib/gstdio.h>
#include <cairo.h>
#include <stdlib.h>
#include <string.h>

static GMainLoop *loop;
static WebKitWebView *view;
static const char *out_path;
static double scale = 1.0;
static int exit_code = 4;

static void finish(int code) { exit_code = code; g_main_loop_quit(loop); }

static gboolean on_timeout(gpointer d) { (void)d; finish(3); return G_SOURCE_REMOVE; }

static void on_links(GObject *src, GAsyncResult *res, gpointer data) {
  cairo_surface_t *surf = data;
  GError *err = NULL;
  JSCValue *v = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(src), res, &err);
  char *json = NULL;
  if (v) { json = jsc_value_to_string(v); g_object_unref(v); }
  if (cairo_surface_write_to_png(surf, out_path) != CAIRO_STATUS_SUCCESS) { finish(4); return; }
  // The page's own backdrop (mail often sets it on a wrapper table, not <body>): the colour of
  // the bottom-left pixel, so the pane can continue it below the image.
  char bg[8] = "#ffffff";
  cairo_surface_flush(surf);
  if (cairo_image_surface_get_format(surf) == CAIRO_FORMAT_ARGB32 || cairo_image_surface_get_format(surf) == CAIRO_FORMAT_RGB24) {
    int h = cairo_image_surface_get_height(surf), w = cairo_image_surface_get_width(surf);
    if (h > 2 && w > 2) {
      const unsigned char *row = cairo_image_surface_get_data(surf) + (h - 2) * cairo_image_surface_get_stride(surf);
      const guint32 px = *(const guint32 *)(row + 4);   // premultiplied ARGB; the page is opaque
      g_snprintf(bg, sizeof bg, "#%02x%02x%02x", (px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
    }
  }
  printf("{\"width\":%d,\"height\":%d,\"scale\":%g,\"bg\":\"%s\",\"links\":%s}\n",
         cairo_image_surface_get_width(surf), cairo_image_surface_get_height(surf),
         scale, bg, json ? json : "[]");
  fflush(stdout);
  // The same line beside the PNG (OUT.json), so the app can keep both in its render cache without re-reading stdout.
  {
    char *jpath = g_strdup_printf("%s.json", out_path);
    char *line = g_strdup_printf("{\"width\":%d,\"height\":%d,\"scale\":%g,\"bg\":\"%s\",\"links\":%s}\n",
        cairo_image_surface_get_width(surf), cairo_image_surface_get_height(surf), scale, bg, json ? json : "[]");
    g_file_set_contents(jpath, line, -1, NULL);
    g_free(line); g_free(jpath);
  }
  g_free(json);
  cairo_surface_destroy(surf);
  finish(0);
}

static void on_snapshot(GObject *src, GAsyncResult *res, gpointer d) {
  (void)d;
  GError *err = NULL;
  cairo_surface_t *surf = webkit_web_view_get_snapshot_finish(WEBKIT_WEB_VIEW(src), res, &err);
  if (!surf) { finish(4); return; }
  const char *js =
    "JSON.stringify(Array.from(document.querySelectorAll('a[href]')).map(function(a){"
    "var r=a.getBoundingClientRect();"
    "return {x:r.left+scrollX,y:r.top+scrollY,w:r.width,h:r.height,href:a.href};})"
    ".filter(function(l){return l.w>0&&l.h>0&&/^(https?|mailto):/i.test(l.href);}))";
  webkit_web_view_evaluate_javascript(WEBKIT_WEB_VIEW(src), js, -1, NULL, NULL, NULL, on_links, surf);
}

static gboolean take_snapshot(gpointer d) {
  (void)d;
  webkit_web_view_get_snapshot(view, WEBKIT_SNAPSHOT_REGION_FULL_DOCUMENT,
                               WEBKIT_SNAPSHOT_OPTIONS_NONE, NULL, on_snapshot, NULL);
  return G_SOURCE_REMOVE;
}

static void on_load(WebKitWebView *v, WebKitLoadEvent ev, gpointer d) {
  (void)v; (void)d;
  if (ev == WEBKIT_LOAD_FINISHED) g_timeout_add(250, take_snapshot, NULL);  // let layout settle
}

static gboolean on_fail(WebKitWebView *v, WebKitLoadEvent e, gchar *u, GError *err, gpointer d) {
  (void)v; (void)e; (void)u; (void)err; (void)d;
  return TRUE;  // a failed subresource is not fatal
}

// Nothing the mail does may navigate away or open windows. The only document ever allowed is our own
// load_html(…, "about:blank"): a <meta http-equiv=refresh> is navigation type OTHER, so checking the type
// is not enough — the page it loads would have no CSP and could script the app's loopback port (SEC-008).
static gboolean on_policy(WebKitWebView *v, WebKitPolicyDecision *dec, WebKitPolicyDecisionType t, gpointer d) {
  (void)v; (void)d;
  if (t == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
    webkit_policy_decision_ignore(dec);
    return TRUE;
  }
  if (t == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
    WebKitNavigationAction *a = webkit_navigation_policy_decision_get_navigation_action(WEBKIT_NAVIGATION_POLICY_DECISION(dec));
    WebKitURIRequest *req = webkit_navigation_action_get_request(a);
    const char *uri = req ? webkit_uri_request_get_uri(req) : NULL;
    if (!uri || strcmp(uri, "about:blank") != 0) {
      webkit_policy_decision_ignore(dec);
      return TRUE;
    }
  }
  return FALSE;
}

// Content-blocker rules (WebKit's url-filter has no alternation, so one rule per pattern). Loopback and
// private networks are never fetched, even with --images: a mail must not reach the app's own port or
// devices on the LAN (SEC-009). Without --images nothing remote is fetched at all, whatever the CSP says.
static const char *RULES_PRIVATE[] = {
  "^[a-z]+://127\\.", "^[a-z]+://0\\.", "^[a-z]+://10\\.", "^[a-z]+://192\\.168\\.", "^[a-z]+://169\\.254\\.",
  "^[a-z]+://172\\.1[6-9]\\.", "^[a-z]+://172\\.2[0-9]\\.", "^[a-z]+://172\\.3[01]\\.",
  "^[a-z]+://localhost[:/]", "^[a-z]+://localhost$", "^[a-z]+://[^/]*\\.localhost[:/]", "^[a-z]+://\\[",
  "^wss?://", NULL,
};
static const char *RULES_NO_REMOTE[] = { "^https?://", "^ftp://", NULL };

static char *rules_json(gboolean images) {
  GString *s = g_string_new("[");
  const char **sets[2] = { RULES_PRIVATE, images ? NULL : RULES_NO_REMOTE };
  gboolean first = TRUE;
  for (int k = 0; k < 2; k++) {
    if (!sets[k]) continue;
    for (const char **p = sets[k]; *p; p++) {
      char *esc = g_strescape(*p, NULL);   // JSON-safe here: the patterns are ASCII with only '\' to escape
      g_string_append_printf(s, "%s{\"trigger\":{\"url-filter\":\"%s\"},\"action\":{\"type\":\"block\"}}", first ? "" : ",", esc);
      g_free(esc);
      first = FALSE;
    }
  }
  g_string_append(s, "]");
  return g_string_free(s, FALSE);
}

static char *filter_dir;   // the compiled rules live in a private temp dir, removed before exit
static char *page_doc;

static void remove_filter_dir(void) {
  if (!filter_dir) return;
  GDir *d = g_dir_open(filter_dir, 0, NULL);
  if (d) {
    const char *n;
    while ((n = g_dir_read_name(d))) { char *p = g_build_filename(filter_dir, n, NULL); g_remove(p); g_free(p); }
    g_dir_close(d);
  }
  g_rmdir(filter_dir);
}

static void on_filter_saved(GObject *src, GAsyncResult *res, gpointer d) {
  (void)d;
  GError *err = NULL;
  WebKitUserContentFilter *f = webkit_user_content_filter_store_save_finish(WEBKIT_USER_CONTENT_FILTER_STORE(src), res, &err);
  if (!f) {   // fail closed: no filter, no render (the UI falls back)
    fprintf(stderr, "content filter: %s\n", err ? err->message : "failed");
    if (err) g_error_free(err);
    finish(4);
    return;
  }
  webkit_user_content_manager_add_filter(webkit_web_view_get_user_content_manager(view), f);
  webkit_user_content_filter_unref(f);
  webkit_web_view_load_html(view, page_doc, "about:blank");
}

static char *read_all(FILE *f, gsize *len) {
  GString *s = g_string_new(NULL);
  char buf[65536]; size_t n;
  while ((n = fread(buf, 1, sizeof buf, f)) > 0) g_string_append_len(s, buf, n);
  *len = s->len;
  return g_string_free(s, FALSE);
}

int main(int argc, char **argv) {
  int width = 700; gboolean images = FALSE;
  for (int i = 1; i < argc; i++) {
    if (!strcmp(argv[i], "--width") && i + 1 < argc) width = atoi(argv[++i]);
    else if (!strcmp(argv[i], "--scale") && i + 1 < argc) scale = atof(argv[++i]);
    else if (!strcmp(argv[i], "--out") && i + 1 < argc) out_path = argv[++i];
    else if (!strcmp(argv[i], "--images")) images = TRUE;
    else { fprintf(stderr, "usage: %s --width N [--scale S] [--images] --out FILE\n", argv[0]); return 2; }
  }
  if (!out_path || width < 100 || width > 4000 || scale < 0.5 || scale > 4) return 2;

  // Software compositing: the offscreen window has no GL context. Must be set before GTK starts.
  g_setenv("WEBKIT_DISABLE_COMPOSITING_MODE", "1", TRUE);
  g_setenv("WEBKIT_DISABLE_DMABUF_RENDERER", "1", TRUE);
  if (!gtk_init_check(&argc, &argv)) { fprintf(stderr, "no display\n"); return 2; }

  gsize len; char *html = read_all(stdin, &len);
  char *csp = g_strdup_printf(
    "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'none'; "
    "img-src data:%s; style-src 'unsafe-inline'; font-src data:\">"
    "<style>html{background:#fff}body{margin:12px;overflow-wrap:anywhere}img{max-width:100%%}</style>",
    images ? " https: http:" : "");
  char *doc = g_strconcat(csp, html, NULL);

  WebKitWebsiteDataManager *mgr = webkit_website_data_manager_new_ephemeral();
  WebKitWebContext *ctx = webkit_web_context_new_with_website_data_manager(mgr);
  webkit_web_context_set_cache_model(ctx, WEBKIT_CACHE_MODEL_DOCUMENT_VIEWER);
  view = WEBKIT_WEB_VIEW(webkit_web_view_new_with_context(ctx));
  WebKitSettings *st = webkit_web_view_get_settings(view);
  // JavaScript stays on only for our own link query (evaluate_javascript); scripts in the mail's markup are
  // off by setting, not just by the CSP (SEC-008).
  webkit_settings_set_enable_javascript(st, TRUE);
  webkit_settings_set_enable_javascript_markup(st, FALSE);
  webkit_settings_set_enable_webgl(st, FALSE);
  webkit_settings_set_enable_media_stream(st, FALSE);
  webkit_settings_set_enable_html5_local_storage(st, FALSE);
  webkit_settings_set_enable_html5_database(st, FALSE);
  webkit_settings_set_media_playback_allows_inline(st, FALSE);
  GdkRGBA white = {1, 1, 1, 1};
  webkit_web_view_set_background_color(view, &white);
  webkit_web_view_set_zoom_level(view, scale);

  GtkWidget *win = gtk_offscreen_window_new();
  gtk_window_set_default_size(GTK_WINDOW(win), (int)(width * scale + 0.5), 200);
  gtk_container_add(GTK_CONTAINER(win), GTK_WIDGET(view));
  gtk_widget_show_all(win);

  g_signal_connect(view, "load-changed", G_CALLBACK(on_load), NULL);
  g_signal_connect(view, "load-failed", G_CALLBACK(on_fail), NULL);
  g_signal_connect(view, "decide-policy", G_CALLBACK(on_policy), NULL);
  g_timeout_add_seconds(20, on_timeout, NULL);

  // Compile the content-blocker rules first; the page is loaded only once they are in place.
  page_doc = doc;
  filter_dir = g_dir_make_tmp("omalook-render-XXXXXX", NULL);
  if (!filter_dir) {   // $TMPDIR unusable: try the per-user runtime dir (private, tmpfs)
    char *tmpl = g_build_filename(g_get_user_runtime_dir(), "omalook-render-XXXXXX", NULL);
    filter_dir = g_mkdtemp_full(tmpl, 0700);
    if (!filter_dir) g_free(tmpl);
  }
  if (!filter_dir) { fprintf(stderr, "content filter: no temp dir\n"); return 4; }
  WebKitUserContentFilterStore *store = webkit_user_content_filter_store_new(filter_dir);
  char *rules = rules_json(images);
  GBytes *rb = g_bytes_new_take(rules, strlen(rules));
  webkit_user_content_filter_store_save(store, "omalook", rb, NULL, on_filter_saved, NULL);
  g_bytes_unref(rb);

  loop = g_main_loop_new(NULL, FALSE);
  g_main_loop_run(loop);
  remove_filter_dir();
  _exit(exit_code);   // skip WebKit teardown: it can take seconds or crash, and we're done
}
