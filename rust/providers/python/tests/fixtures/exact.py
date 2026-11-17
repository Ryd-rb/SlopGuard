def combine(value, count):
    running_total = value + count
    if running_total > 5:
        running_total = running_total * 2
    return running_total
