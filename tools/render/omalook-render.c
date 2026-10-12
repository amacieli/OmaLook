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
 * Isolation: ephemeral data manager (no cookies, cache or storage), page scripts blocked by a
 * content-security-policy and by disabling JavaScript for the page, navigation and popups denied,
 * and nothing remote is fetched unless --images is given.
 */
#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
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

// Nothing the mail does may navigate away or open windows.
static gboolean on_policy(WebKitWebView *v, WebKitPolicyDecision *dec, WebKitPolicyDecisionType t, gpointer d) {
  (void)v; (void)d;
  if (t == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION || t == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
    WebKitNavigationAction *a = webkit_navigation_policy_decision_get_navigation_action(WEBKIT_NAVIGATION_POLICY_DECISION(dec));
    if (t == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION ||
        webkit_navigation_action_get_navigation_type(a) != WEBKIT_NAVIGATION_TYPE_OTHER) {
      webkit_policy_decision_ignore(dec);
      return TRUE;
    }
  }
  return FALSE;
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
  webkit_settings_set_enable_javascript(st, TRUE);   // for our own link query; the page's are blocked by CSP
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

  webkit_web_view_load_html(view, doc, "about:blank");
  loop = g_main_loop_new(NULL, FALSE);
  g_main_loop_run(loop);
  _exit(exit_code);   // skip WebKit teardown: it can take seconds or crash, and we're done
}
