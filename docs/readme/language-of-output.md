# Language of output

The CLI, hook, and installer guidance output is available in 9 languages (ja, en, zh-CN, zh-TW, ko, es, fr, de, pt-BR). The language is chosen in this order: `SCRIPTURA_LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG` → the OS display language → English. The admin UI uses its own selection order (described above). JSON keys and values meant for machines (such as `status`, `outcome`, and `doctor`'s `id`) are not translated.
