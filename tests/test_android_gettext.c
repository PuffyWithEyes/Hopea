// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
// Native emulator smoke test: LIBRARY CATALOG_DIRECTORY LANGUAGE.
#include <assert.h>
#include <dlfcn.h>
#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
 assert(argc == 4);
 const char *lang=argv[3];
 setenv("LANGUAGE",lang,1);setenv("LC_MESSAGES","en_US.UTF-8",1);unsetenv("LC_ALL");setlocale(LC_ALL,"");
 void *b=dlopen(argv[1],RTLD_NOW|RTLD_GLOBAL);assert(b);
 void *p=dlopen("libintl.so",RTLD_NOW|RTLD_GLOBAL);assert(p);
 int (*init)(void*)=dlsym(p,"g_libintl_use_backend");assert(init && init(b));
 char *(*bind)(const char*,const char*)=dlsym(b,"libintl_bindtextdomain");
 char *(*domain)(const char*)=dlsym(b,"libintl_textdomain");
 char *(*codeset)(const char*,const char*)=dlsym(b,"libintl_bind_textdomain_codeset");
 char *(*get)(const char*)=dlsym(b,"libintl_gettext");
 char *(*plural)(const char*,const char*,unsigned long)=dlsym(p,"g_libintl_ngettext");
 bind("hopea",argv[2]);domain("hopea");codeset("hopea","UTF-8");
 const char *expected=strcmp(lang,"ru")==0?"Для вас":"For You";
 assert(strcmp(get("For You"),expected)==0);
 void *g=dlopen("libglib-2.0.so",RTLD_NOW|RTLD_GLOBAL);if(!g){puts(dlerror());return 2;}
 char *(*gtkget)(const char*,const char*)=dlsym(g,"g_dgettext");
 assert(strcmp(gtkget("hopea","For You"),expected)==0);
 assert(strcmp(gtkget("hopea","Untranslated fixture"),"Untranslated fixture")==0);
 if(strcmp(lang,"ru")==0){
   assert(strcmp(plural("on {} computer","on {} computers",1),"на {} компьютере")==0);
   assert(strcmp(plural("on {} computer","on {} computers",2),"на {} компьютерах")==0);
 }
 printf("Android Rust/GLib gettext, plural and missing-message fallback: %s OK\n",lang);
 fflush(stdout);_exit(0);
}
