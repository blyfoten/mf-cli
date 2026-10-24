// SPDX-License-Identifier: GPL-2.0-only
/*
 * Copyright (c) 2026 Pavel Olizko <contact@nolight.dev>
 * Copyright (c) 2026 sleebu <contact@sleebu.dev>
 */
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <linux/usb.h>

#define MINIFUSE_VID 0x1c75

#define MF_REQ_SET 0x22
#define MF_REQ_GET_BLOCK 0xb2
#define MF_BLOCK_LEN 190
#define MF_REQ_GET_METERS 0xc2
#define MF_METERS_LEN 64

/*
 * Interface version of /dev/minifuse_cmd, read by mf-cli:
 *   1: write "<selector hex> <0|1>"
 *   2: write "<wValue hex> <16-bit value>", read returns the settings block
 *   3: as 2, and reading /dev/minifuse_meters returns the level meters
 */
static int api_version = 3;
module_param(api_version, int, 0444);
MODULE_PARM_DESC(api_version, "Interface version of /dev/minifuse_cmd");

static int match_minifuse(struct usb_device *udev, void *data) {
  struct usb_device **found = data;

  if (le16_to_cpu(udev->descriptor.idVendor) == MINIFUSE_VID) {
    *found = usb_get_dev(udev);
    return 1;
  }
  return 0;
}

static struct usb_device *find_minifuse(void) {
  struct usb_device *udev = NULL;

  usb_for_each_dev(&udev, match_minifuse);
  if (!udev)
    printk(KERN_ERR "MiniFuse: Device not found\n");
  return udev;
}

static ssize_t mftoggle_write(struct file *file, const char __user *user_buf,
                              size_t count, loff_t *ppos) {
  char buf[32];
  size_t len = min(count, sizeof(buf) - 1);
  struct usb_device *udev;
  unsigned short wvalue = 0;
  unsigned short value = 0;
  u8 *data;
  int ret;

  // Safely copy the string from userspace to kernel space
  if (copy_from_user(buf, user_buf, len))
    return -EFAULT;
  buf[len] = '\0';

  if (sscanf(buf, "%hx %hu", &wvalue, &value) != 2)
    return -EINVAL;

  udev = find_minifuse();
  if (!udev)
    return -ENODEV;

  data = kmalloc(2, GFP_KERNEL);
  if (!data) {
    usb_put_dev(udev);
    return -ENOMEM;
  }

  // Little-endian 16-bit value; on/off is 1/0
  data[0] = value & 0xff;
  data[1] = value >> 8;

  ret = usb_control_msg(udev, usb_sndctrlpipe(udev, 0), MF_REQ_SET, 0x21,
                        wvalue, 0, data, 2, 200);

  kfree(data);
  usb_put_dev(udev);

  if (ret < 0) {
    printk(KERN_ERR "MiniFuse: usb_control_msg failed (%d)\n", ret);
    return ret;
  }

  return count;
}

// One read returns the whole reply to a GET request, then end of file
static ssize_t mf_read_request(u8 request, size_t reply_len,
                               char __user *user_buf, size_t count,
                               loff_t *ppos) {
  struct usb_device *udev;
  size_t len;
  u8 *data;
  int ret;

  if (*ppos > 0)
    return 0;

  udev = find_minifuse();
  if (!udev)
    return -ENODEV;

  data = kmalloc(reply_len, GFP_KERNEL);
  if (!data) {
    usb_put_dev(udev);
    return -ENOMEM;
  }

  ret = usb_control_msg(udev, usb_rcvctrlpipe(udev, 0), request, 0xa1, 0, 0,
                        data, reply_len, 500);
  usb_put_dev(udev);

  if (ret < 0) {
    printk(KERN_ERR "MiniFuse: usb_control_msg failed (%d)\n", ret);
    kfree(data);
    return ret;
  }

  len = min_t(size_t, ret, count);
  if (copy_to_user(user_buf, data, len)) {
    kfree(data);
    return -EFAULT;
  }

  kfree(data);
  *ppos += len;
  return len;
}

static ssize_t mftoggle_read(struct file *file, char __user *user_buf,
                             size_t count, loff_t *ppos) {
  return mf_read_request(MF_REQ_GET_BLOCK, MF_BLOCK_LEN, user_buf, count, ppos);
}

static ssize_t mfmeters_read(struct file *file, char __user *user_buf,
                             size_t count, loff_t *ppos) {
  return mf_read_request(MF_REQ_GET_METERS, MF_METERS_LEN, user_buf, count,
                         ppos);
}

static const struct file_operations mf_fops = {
    .owner = THIS_MODULE,
    .write = mftoggle_write,
    .read = mftoggle_read,
};

static struct miscdevice mf_misc = {
    .minor = MISC_DYNAMIC_MINOR,
    .name = "minifuse_cmd",
    .fops = &mf_fops,
    .mode = 0666, // Read for the settings block, write for commands
};

static const struct file_operations mf_meters_fops = {
    .owner = THIS_MODULE,
    .read = mfmeters_read,
};

static struct miscdevice mf_meters_misc = {
    .minor = MISC_DYNAMIC_MINOR,
    .name = "minifuse_meters",
    .fops = &mf_meters_fops,
    .mode = 0444,
};

static int __init mf_init(void) {
  int ret = misc_register(&mf_misc);

  if (ret)
    return ret;
  ret = misc_register(&mf_meters_misc);
  if (ret)
    misc_deregister(&mf_misc);
  return ret;
}

static void __exit mf_exit(void) {
  misc_deregister(&mf_meters_misc);
  misc_deregister(&mf_misc);
}

module_init(mf_init);
module_exit(mf_exit);
MODULE_LICENSE("GPL");
MODULE_AUTHOR("Pavel Olizko <contact@nolight.dev>");
MODULE_AUTHOR("sleebu <contact@sleebu.dev>");
MODULE_DESCRIPTION("Bypass usbfs to send control commands to Arturia MiniFuse");
