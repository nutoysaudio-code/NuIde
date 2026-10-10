/* generado por spasm-c-gen (no editar a mano) */
/* programa "nucoder_codeassist" */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

const char * codeassist_out;

static int64_t spasm_func_keywords_for(const char * lang);

static int64_t spasm_func_keywords_for(const char * lang)
{
    if (((strcmp(lang, "spasm") == 0) == 1)) {
        codeassist_out = "print,input,var,const,func,if,while,for,select,case,default,buffer,spawn,wait,channel_set,channel_get,str_len,str_equals,str_slice,fl_sqrt,fl_sin,fl_cos";
        return 1;
    }
    if (((strcmp(lang, "rust") == 0) == 1)) {
        codeassist_out = "fn,let,mut,const,struct,enum,impl,trait,match,if,else,loop,while,for,return,use,mod,pub,Vec,String,Option,Result,HashMap";
        return 1;
    }
    if (((strcmp(lang, "python") == 0) == 1)) {
        codeassist_out = "def,class,import,from,as,if,elif,else,for,while,try,except,finally,with,return,yield,lambda,list,dict,set,tuple";
        return 1;
    }
    codeassist_out = "if,else,for,while,return,function,const,let";
    return 1;
}

int main(void)
{
    codeassist_out = "";
    return 0;
}

