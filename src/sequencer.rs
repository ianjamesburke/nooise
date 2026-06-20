pub(crate) fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }

    left
}

pub(crate) fn lcm(left: u32, right: u32) -> u32 {
    left / gcd(left, right) * right
}
