#include <chrono>
#include <thread>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <utility>
#include <cstring>
#include <atomic>
#include <csignal>

#include "uinput_helper.hpp"

extern "C" {
    #include <linux/uinput.h>
    #include <unistd.h>
    #include <fcntl.h>
}

#define IOCTL_WRAPPER(call) \
    ({ \
        typeof(call) ret = (call); \
        if (ret < 0) { \
            fprintf(stderr, "IOCTL failed at %s:%d: %s\n", __FILE__, __LINE__, strerror(errno)); \
            exit(EXIT_FAILURE); \
        } \
        ret; \
    })




static std::string find_event_node(const std::string& sysfs_input_path)
{
    DIR *dir = opendir(sysfs_input_path.c_str());
    if (!dir)
        throw std::runtime_error("opendir failed");

    struct dirent *ent;
    while ((ent = readdir(dir)) != nullptr) {
        if (strncmp(ent->d_name, "event", 5) == 0) {
            std::string path = "/dev/input/";
            path += ent->d_name;
            closedir(dir);
            return path;
        }
    }

    closedir(dir);
    throw std::runtime_error("no event node found");
}


// Generated via JSON.stringify(Object.values(window.key_mapping)), will be the only usable keys. 
auto static kb_buttons_bound = {
    2,3,4,5,6,7,8,9,10,11,16,17,18,19,20,21,22,23,24,25,30,31,32,33,
    34,35,36,37,38,44,45,46,47,48,49,50,103,105,106,108,1,12,13,14,
    15,26,27,28,29,39,40,41,42,43,51,52,53,54,56,57,58,59,60,61,62,
    63,64,65,66,67,68,87,88,69,70,55,71,72,73,74,75,76,77,78,79,80,
    81,82,83,96,97,98,100,102,104,107,109,110,111,272,273,274
};


std::string setup_uinput_keyboard_mouse(stateData *data) {
    data->uinput_kbm_fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (data->uinput_kbm_fd < 0) {
        perror("Damn no uinput");
        // return false;
        throw std::runtime_error("No uinput ability");
    }
    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_EVBIT, EV_REL));
    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_RELBIT, REL_X));
    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_RELBIT, REL_Y));

    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_RELBIT, REL_WHEEL_HI_RES));


    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_EVBIT, EV_SYN));

    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_EVBIT, EV_KEY));
    

    for (uint16_t button: kb_buttons_bound) {
        IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_SET_KEYBIT, button));
    }


    struct uinput_setup usetup = {0};
    
    usetup.id.bustype = BUS_USB;
    usetup.id.vendor = 0x1234;
    usetup.id.product = 0x5678;
    strcpy(usetup.name, "Example dev!");

    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_DEV_SETUP, &usetup));
    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_DEV_CREATE));

    char sysfs_device_name[16];
    IOCTL_WRAPPER(ioctl(data->uinput_kbm_fd, UI_GET_SYSNAME(sizeof(sysfs_device_name)), sysfs_device_name));
    std::string device_name = std::string(sysfs_device_name);
    std::string dev_event_id = find_event_node("/sys/devices/virtual/input/"+device_name);
    std::cout << "Created device: "+dev_event_id << std::endl;

    return dev_event_id;
}

static void emit_uinput(int fd, int type, int code, int val)
{
   struct input_event ie;

   ie.type = type;
   ie.code = code;
   ie.value = val;
   /* timestamp values below are ignored */
   ie.time.tv_sec = 0;
   ie.time.tv_usec = 0;

   ssize_t n = write(fd, &ie, sizeof(ie));
   if (n<0) {
        perror("Failed to write to uinput");
   }
}

void process_remote_message(stateData *data, std::vector<std::byte> bin_data) {

    if (bin_data.size() >= 6) {
        int16_t x_movement = read_le_from_vec<int16_t>(bin_data,1);
        int16_t y_movement = read_le_from_vec<int16_t>(bin_data,3);
        int16_t scroll_movement = read_le_from_vec<int16_t>(bin_data,5);

        emit_uinput(data->uinput_kbm_fd, EV_REL, REL_X, x_movement);
        emit_uinput(data->uinput_kbm_fd, EV_REL, REL_Y, y_movement);
        emit_uinput(data->uinput_kbm_fd, EV_REL, REL_WHEEL_HI_RES, scroll_movement);
        for (int i=7; i<bin_data.size(); i+=2) {
            // EV_KEY
            auto data_byte = read_le_from_vec<uint16_t>(bin_data,i);
            bool pressed = ((data_byte&0x1000)>0);

            emit_uinput(data->uinput_kbm_fd, EV_KEY, data_byte&0xFFF, pressed);
        }

        emit_uinput(data->uinput_kbm_fd, EV_SYN, SYN_REPORT, 0);
    }
}