/* FX2LP local prototype: EP6 parallel AUTOIN -> PC; EP2 -> PA0 UART -> Basic.
 * RAM-only image. No EEPROM writes. USB identity remains board's 04b4:8613.
 * Register addresses and FIFO timings: Infineon FX2LP TRM / datasheet.
 */
#include <stdint.h>
#define REG(name, address) __xdata __at(address) volatile uint8_t name
REG(CPUCS,0xe600); REG(IFCONFIG,0xe601); REG(PINFLAGSAB,0xe602);
REG(PINFLAGSCD,0xe603); REG(FIFORESET,0xe604); REG(FIFOPINPOLAR,0xe609);
REG(REVCTL,0xe60b); REG(EP1OUTCFG,0xe610); REG(EP1INCFG,0xe611);
REG(EP2CFG,0xe612); REG(EP4CFG,0xe613); REG(EP6CFG,0xe614); REG(EP8CFG,0xe615);
REG(EP2FIFOCFG,0xe618); REG(EP6FIFOCFG,0xe61a);
REG(EP6AUTOINLENH,0xe624); REG(EP6AUTOINLENL,0xe625);
REG(USBIRQ,0xe65d); REG(USBIE,0xe65c); REG(PORTACFG,0xe670);
REG(USBCS,0xe680); REG(EP0BCH,0xe68a); REG(EP0BCL,0xe68b);
REG(EP2BCH,0xe690); REG(EP2BCL,0xe691); REG(EP0CS,0xe6a0); REG(EP2CS,0xe6a3);
REG(EP6CS,0xe6a5);
REG(EP6FIFOBCH,0xe6af); REG(EP6FIFOBCL,0xe6b0); REG(EP6FIFOFLGS,0xe6a9);
REG(GPIFREADYSTAT,0xe6f4);
__xdata __at(0xe6b8) volatile uint8_t SETUPDAT[8];
__xdata __at(0xe740) volatile uint8_t EP0BUF[64];
__xdata __at(0xf000) volatile uint8_t EP2BUF[512];
__sfr __at(0x80) IOA;
__sfr __at(0xb2) OEA;
__sfr __at(0xa8) IE;
__sfr __at(0x90) IOB;
__sbit __at(0x80) RETURN_TX;
static uint8_t configured;
static uint16_t uart_bytes, out_packets;
static void syncdelay(void) {
    __asm
    nop
    nop
    nop
    nop
    nop
    nop
    nop
    nop
    __endasm;
}
// Generated instruction-counted 230769 baud UART at 48MHz / four clocks.
void return_byte(uint8_t value) __naked;
#include "return_uart.inc"

static const __code uint8_t device[] = {
    18,1,0,2,0,0,0,64,0xb4,0x04,0x13,0x86,0,1,1,2,0,1
};
static const __code uint8_t qualifier[] = {10,6,0,2,0,0,0,64,1,0};
static const __code uint8_t config_hs[] = {
    9,2,32,0,1,1,0,0x80,50, 9,4,0,0,2,0xff,0,0,0,
    7,5,2,2,0,2,0, 7,5,0x86,2,0,2,0
};
static const __code uint8_t config_fs[] = {
    9,2,32,0,1,1,0,0x80,50, 9,4,0,0,2,0xff,0,0,0,
    7,5,2,2,64,0,0, 7,5,0x86,2,64,0,0
};
static const __code uint8_t language[] = {4,3,9,4};
static const __code uint8_t manufacturer[] = {16,3,'C',0,'5',0,'P',0,'l',0,'a',0,'y',0,' ',0};
static const __code uint8_t product[] = {24,3,'B',0,'a',0,'s',0,'i',0,'c',0,' ',0,'F',0,'X',0,'2',0,'L',0,'P',0};
static const __code uint8_t ms_string[] = {18,3,'M',0,'S',0,'F',0,'T',0,'1',0,'0',0,'0',0,0x20,0};
static const __code uint8_t ms_compat[] = {
    40,0,0,0,0,1,4,0,1,0,0,0,0,0,0,0,
    0,1,'W','I','N','U','S','B',0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0
};
#include "winusb_properties.inc"
static uint16_t word(uint8_t pos) { return SETUPDAT[pos] | ((uint16_t)SETUPDAT[pos+1] << 8); }
static void reply(const __code uint8_t *data, uint16_t count)
{
    uint16_t wanted = word(6), offset = 0, guard;
    uint8_t n, i;
    if (count > wanted) count = wanted;
    while (offset < count) {
        guard = 60000;
        while (EP0CS & 2) if (!--guard || (USBIRQ & 1)) return;
        n = count - offset > 64 ? 64 : count - offset;
        for (i=0;i<n;++i) EP0BUF[i] = data[offset+i];
        EP0BCH=0; EP0BCL=n;
        offset += n;
    }
    EP0CS |= 0x80;
}
static void fifo_reset(void)
{
    EP6FIFOCFG=0; syncdelay();
    FIFORESET=0x80; syncdelay();
    FIFORESET=2; syncdelay(); FIFORESET=6; syncdelay();
    FIFORESET=0; syncdelay();
    EP6AUTOINLENH=2; syncdelay(); EP6AUTOINLENL=0; syncdelay();
    EP6FIFOCFG=8; syncdelay(); // Arm AUTOIN after each reset/configuration.
    EP2BCL=0; syncdelay(); EP2BCL=0; syncdelay();
}
static void setup(void)
{
    uint8_t request = SETUPDAT[1], type=SETUPDAT[0], descriptor=SETUPDAT[3];
    static const __code uint8_t zero[] = {0,0};
    USBIRQ=1;
    if (type == 0x80 && request == 6) {
        if (descriptor==1) reply(device,sizeof(device));
        else if (descriptor==2) reply((USBCS&0x80)?config_hs:config_fs,32);
        else if (descriptor==6) reply(qualifier,sizeof(qualifier));
        else if (descriptor==3) {
            switch (SETUPDAT[2]) {
            case 0: reply(language,sizeof(language)); break;
            case 1: reply(manufacturer,sizeof(manufacturer)); break;
            case 2: reply(product,sizeof(product)); break;
            case 0xee: reply(ms_string,sizeof(ms_string)); break;
            default: EP0CS|=1; break;
            }
        } else EP0CS|=1;
    } else if (type==0xc0 && request==0x20 && word(4)==4) {
        reply(ms_compat,sizeof(ms_compat));
    } else if (type==0xc1 && request==0x20 && word(4)==5) {
        reply(ms_properties,sizeof(ms_properties));
    } else if (type==0xc0 && request==0xb0) {
        EP0BUF[0]='B'; EP0BUF[1]='5'; EP0BUF[2]='F'; EP0BUF[3]='X';
        EP0BUF[4]=1; EP0BUF[5]=(USBCS&0x80)?1:0; EP0BUF[6]=configured; EP0BUF[7]=0;
        EP0BCH=0; EP0BCL=8; EP0CS|=0x80;
    } else if (type==0xc0 && request==0xb1 && word(6)>=12) {
        EP0BUF[0]=CPUCS; EP0BUF[1]=IFCONFIG; EP0BUF[2]=EP2CS; EP0BUF[3]=EP6CS;
        EP0BUF[4]=IOA; EP0BUF[5]=OEA; EP0BUF[6]=PINFLAGSAB; EP0BUF[7]=FIFOPINPOLAR;
        EP0BUF[8]=(uint8_t)uart_bytes; EP0BUF[9]=(uint8_t)(uart_bytes>>8);
        EP0BUF[10]=(uint8_t)out_packets; EP0BUF[11]=(uint8_t)(out_packets>>8);
        EP0BUF[12]=EP6FIFOBCH; EP0BUF[13]=EP6FIFOBCL; EP0BUF[14]=EP6FIFOFLGS; EP0BUF[15]=IOB;
        EP0BUF[16]=EP6FIFOCFG; EP0BUF[17]=EP6AUTOINLENH; EP0BUF[18]=EP6AUTOINLENL; EP0BUF[19]=EP6CFG;
        EP0BUF[20]=GPIFREADYSTAT; EP0BUF[21]=PORTACFG; EP0BUF[22]=0; EP0BUF[23]=0;
        EP0BCH=0; EP0BCL=word(6)>=24 ? 24 : word(6)>=20 ? 20 : word(6)>=16 ? 16 : 12; EP0CS|=0x80;
    } else if (type==0 && request==9 && word(2)<=1 && word(6)==0) {
        configured=SETUPDAT[2]; fifo_reset(); EP0CS|=0x80;
    } else if (type==0x80 && request==8) {
        EP0BUF[0]=configured; EP0BCH=0; EP0BCL=1; EP0CS|=0x80;
    } else if ((type==0x80 || type==0x81 || type==0x82) && request==0) {
        reply(zero,2);
    } else if (type==0x81 && request==10 && word(4)==0) {
        reply(zero,1);
    } else if (type==1 && request==11 && word(2)==0 && word(4)==0) {
        EP0CS|=0x80;
    } else EP0CS|=1;
}
void main(void)
{
    uint16_t i, length;
    IE=0; CPUCS=0x10; syncdelay(); // 48MHz CPU, CLKOUT disabled
    RETURN_TX=1; OEA=1; PORTACFG=0x40; // PA7 is SLCS#, explicitly selected by GND.
    USBCS |= 0x0a; // disconnect and serve descriptors from RAM firmware
    IFCONFIG=0xcb; syncdelay(); // internal 48MHz, asynchronous 8bit slave FIFO
    FIFOPINPOLAR=0; syncdelay();
    PINFLAGSAB=0x0e; syncdelay(); // CTL0 = fixed EP6 full, active low
    PINFLAGSCD=0; syncdelay();
    EP1OUTCFG=0; EP1INCFG=0; EP4CFG=0; EP8CFG=0;
    EP2CFG=0xa2; syncdelay(); // bulk OUT 512, double bank, CPU drained
    EP6CFG=0xe0; syncdelay(); // bulk IN 512, quad bank
    REVCTL=3; syncdelay();
    EP2FIFOCFG=0; syncdelay(); EP6FIFOCFG=0; syncdelay();
    EP6AUTOINLENH=2; syncdelay(); EP6AUTOINLENL=0; syncdelay();
    EP6FIFOCFG=8; syncdelay(); // AUTOIN, 8bit. Basic always sends whole 512B.
    fifo_reset(); USBIE=0; USBIRQ=0xff;
    for (i=0;i<60000;++i) syncdelay();
    USBCS &= (uint8_t)~8;
    for (;;) {
        if (USBIRQ & 0x10) { USBIRQ=0x10; configured=0; fifo_reset(); }
        if (USBIRQ & 1) setup();
        // EP2CS EMPTY is bit2. Bit1 is reserved, unlike EP0CS BUSY bit1.
        if (configured && !(EP2CS&4)) {
            length=((uint16_t)EP2BCH<<8)|EP2BCL;
            if (length<=512) {
                for (i=0;i<length;++i) {
                    return_byte(EP2BUF[i]);
                    uart_bytes++;
                    if (USBIRQ&1) setup();
                }
            }
            out_packets++;
            EP2BCL=0; syncdelay();
        }
    }
}
