#include <stdio.h>
#include "pico/stdlib.h"
int main(void) {
    stdio_init_all();
    for (unsigned count=0;;++count) {
        printf("OpenWhoop USB probe heartbeat %u\n",count);
        sleep_ms(1000);
    }
}
