#ifndef _LWIPOPTS_H
#define _LWIPOPTS_H

// Minimal lwIP options for the OpenWhoop Pico W relay spike (NO_SYS mode).
#define NO_SYS 1
#define LWIP_SOCKET 0
#define LWIP_NETCONN 0
#define LWIP_DHCP 1
#define LWIP_DNS 1
#define LWIP_NETIF_HOSTNAME 1
#define LWIP_NETIF_STATUS_CALLBACK 1
#define LWIP_IGMP 1
#define LWIP_RAW 1
#define LWIP_TCP 1
#define LWIP_ALTCP 1

#endif
