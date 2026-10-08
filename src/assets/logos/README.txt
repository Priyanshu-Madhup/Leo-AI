Brand logos shown next to Leo's tool steps.

Put image files here (png, svg, webp, jpg). The file name, without the
extension, is the key. Leo uses the first key that exists; if none does it draws
a neutral icon. Use lowercase names.

Services (the Google ones fall back to google.* if their own is missing):
  google             general Google logo, used when a product logo is missing
  gmail
  google-calendar
  google-docs
  google-sheets
  google-slides
  google-drive
  google-contacts

Built-in steps:
  web-search         "Searching the web"
  memory             "Checking memory" / "Saving to memory"
  clock              "Checking the time"

Websites Leo reads or opens (named after the site; a parent domain also works):
  wikipedia.org  (or just: wikipedia)
  github.com     (or just: github)
  ... any site you want a logo for

Apps Leo opens (the app's name, lowercase, spaces as dashes):
  spotify
  visual-studio-code
  ... any app you want a logo for

Square images around 80x80 px or larger look best. They are shown inside a
white circle, so logos with transparent backgrounds work well.

The app's own icon:
  leo-app-icon.png   the source artwork for Leo's icon (installer, taskbar,
                     window). It is NOT used as a tool-row logo. To change the
                     icon, replace this file, crop/round it into
                     src-tauri/icons/app-icon-source.png (1024x1024, see
                     detailed_readme.md section 16), then run
                       npx tauri icon src-tauri/icons/app-icon-source.png
                     and delete the android and ios folders it creates.
