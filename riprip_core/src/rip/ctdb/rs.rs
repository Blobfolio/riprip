//! Reed–Solomon decoding over CTDB's GF(2^16).

/// Number of nonzero field elements.
const ORDER: usize = 65_535;
/// CTDB's field polynomial, including its leading term.
const POLYNOMIAL: u32 = 0x1_100b;
/// Largest syndrome record supported by the CTDB service.
pub(super) const MAX_PARITY: usize = 16;

/// Field arithmetic with primitive element 2.
pub(super) struct Field {
	/// Discrete logarithms to base two.
	log: Vec<usize>,
	/// Powers of two, repeated to allow a lookup without reduction.
	exp: Vec<u16>,
}

impl Field {
	/// Generate lookup tables using the CTDB primitive polynomial.
	pub(super) fn new() -> Self {
		let mut log = vec![0; ORDER + 1];
		let mut exp = Vec::with_capacity(2 * ORDER);
		let mut value = 1_u32;
		for power in 0..ORDER {
			let element = u16::try_from(value).expect("reduced field element");
			log[usize::from(element)] = power;
			exp.push(element);
			value <<= 1;
			if value & 0x1_0000 != 0 { value ^= POLYNOMIAL; }
		}
		exp.extend_from_within(..);
		Self { log, exp }
	}

	/// Evaluate the primitive element to the given power.
	pub(super) fn alpha(&self, power: usize) -> u16 { self.exp[power % ORDER] }

	/// Multiply by a power of the primitive element.
	pub(super) fn mul_alpha(&self, value: u16, power: usize) -> u16 {
		if value == 0 { 0 }
		else { self.exp[self.log[usize::from(value)] + power % ORDER] }
	}

	/// Multiply field elements.
	fn mul(&self, a: u16, b: u16) -> u16 {
		if a == 0 || b == 0 { 0 }
		else { self.exp[self.log[usize::from(a)] + self.log[usize::from(b)]] }
	}

	/// Divide by a nonzero field element.
	fn div(&self, a: u16, b: u16) -> u16 {
		debug_assert_ne!(b, 0, "field division by zero");
		if a == 0 { 0 }
		else { self.exp[ORDER + self.log[usize::from(a)] - self.log[usize::from(b)]] }
	}

	/// A fixed field multiplication can be evaluated with two byte lookups.
	pub(super) fn multiplier(&self, factor: u16) -> [[u16; 256]; 2] {
		std::array::from_fn(|byte| {
			std::array::from_fn(|value| {
				self.mul(factor, u16::try_from(value).expect("one byte") << (byte * 8))
			})
		})
	}

	/// Find errors in a shortened codeword whose first word has degree
	/// `words - 1`. Return (position, XOR magnitude) for changed words.
	pub(super) fn decode(
		&self,
		syndromes: &[u16],
		words: usize,
		erasures: &[usize],
	) -> Option<Vec<(usize, u16)>> {
		if syndromes.iter().all(|&s| s == 0) { return Some(Vec::new()); }
		if erasures.len() > syndromes.len() { return None; }

		// For each known location X, taking S[j+1] + X*S[j] cancels
		// its contribution and leaves a shorter sequence for the unknowns.
		let mut reduced = [0; MAX_PARITY];
		let mut remaining = syndromes.len();
		reduced[..remaining].copy_from_slice(syndromes);
		let mut positions = Vec::from(erasures);
		for &position in erasures {
			let x = self.alpha(words - 1 - position);
			for j in 0..remaining - 1 {
				reduced[j] = reduced[j + 1] ^ self.mul(x, reduced[j]);
			}
			remaining -= 1;
		}

		let (locator, degree) = self.recurrence(&reduced[..remaining]);
		if 2 * degree > remaining || locator[degree] == 0 { return None; }
		if degree == 1 {
			let power = self.log[usize::from(locator[1])];
			if power >= words { return None; }
			positions.push(words - 1 - power);
		}
		else if degree > 1 {
			if !self.splits(&locator[..=degree]) { return None; }
			for power in 0..words {
				let inverse = self.alpha(ORDER - power);
				let value = locator[..=degree].iter().rev().fold(0, |sum, &term| {
					self.mul(sum, inverse) ^ term
				});
				if value == 0 {
					positions.push(words - 1 - power);
					if positions.len() == erasures.len() + degree { break; }
				}
			}
		}
		positions.sort_unstable();
		if positions.len() != erasures.len() + degree ||
			positions.windows(2).any(|pair| pair[0] == pair[1])
		{
			return None;
		}

		// With the locations fixed, S[j] = sum(magnitude[i] * X[i]^j).
		// Solve the small Vandermonde system and check every syndrome.
		let mut matrix = [[0; MAX_PARITY + 1]; MAX_PARITY];
		let count = positions.len();
		for (row, equation) in matrix[..count].iter_mut().enumerate() {
			for (col, &position) in positions.iter().enumerate() {
				equation[col] = self.alpha(row * (words - 1 - position));
			}
			equation[count] = syndromes[row];
		}
		for pivot in 0..count {
			let row = (pivot..count).find(|&row| matrix[row][pivot] != 0)?;
			matrix.swap(pivot, row);
			let divisor = matrix[pivot][pivot];
			for value in &mut matrix[pivot][pivot..=count] {
				*value = self.div(*value, divisor);
			}
			let normalized = matrix[pivot];
			for (row, equation) in matrix[..count].iter_mut().enumerate() {
				if row == pivot { continue; }
				let factor = equation[pivot];
				for col in pivot..=count {
					equation[col] ^= self.mul(factor, normalized[col]);
				}
			}
		}
		let changes: Vec<_> = positions.into_iter().zip(matrix.iter().map(|row| row[count]))
			.filter(|&(_, magnitude)| magnitude != 0).collect();
		for (root, &expected) in syndromes.iter().enumerate() {
			let actual = changes.iter().fold(0, |sum, &(position, magnitude)| {
				sum ^ self.mul(magnitude, self.alpha(root * (words - 1 - position)))
			});
			if actual != expected { return None; }
		}
		Some(changes)
	}

	/// Every valid locator divides x^(2^16) + x. Check this invariant before
	/// scanning thousands of possible positions for a wrong pressing alignment.
	fn splits(&self, locator: &[u16]) -> bool {
		let degree = locator.len() - 1;
		let mut polynomial = [0; MAX_PARITY];
		polynomial[1] = 1;
		let mut modulus = [0; MAX_PARITY];
		for (out, &term) in modulus[..degree].iter_mut().zip(locator) {
			*out = self.div(term, locator[degree]);
		}
		for _ in 0..16 {
			let mut square = [0; 2 * MAX_PARITY];
			for (i, &value) in polynomial[..degree].iter().enumerate() {
				square[2 * i] = self.mul(value, value);
			}
			for i in (degree..2 * degree - 1).rev() {
				let factor = square[i];
				for (term, &coefficient) in square[i - degree..i].iter_mut().zip(&modulus) {
					*term ^= self.mul(factor, coefficient);
				}
			}
			polynomial[..degree].copy_from_slice(&square[..degree]);
		}
		polynomial[1] ^= 1;
		polynomial[..degree].iter().all(|&term| term == 0)
	}

	/// Berlekamp–Massey shortest recurrence for a sequence of field elements.
	fn recurrence(&self, sequence: &[u16]) -> ([u16; MAX_PARITY + 1], usize) {
		let mut current = [0; MAX_PARITY + 1];
		current[0] = 1;
		let mut previous = current;
		let mut degree = 0;
		let mut distance = 1;
		let mut last = 1;
		for (n, &symbol) in sequence.iter().enumerate() {
			let discrepancy = (1..=degree).fold(symbol, |sum, i| {
				sum ^ self.mul(current[i], sequence[n - i])
			});
			if discrepancy == 0 {
				distance += 1;
				continue;
			}

			let before = current;
			let scale = self.div(discrepancy, last);
			for (term, &prior) in current[distance..].iter_mut().zip(previous.iter()) {
				*term ^= self.mul(scale, prior);
			}
			if degree * 2 <= n {
				previous = before;
				degree = n + 1 - degree;
				last = discrepancy;
				distance = 1;
			}
			else { distance += 1; }
		}
		(current, degree)
	}
}
