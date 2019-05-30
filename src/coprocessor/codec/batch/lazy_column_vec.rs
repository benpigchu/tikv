// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use tipb::expression::FieldType;

use super::LazyBatchColumn;
use crate::coprocessor::codec::data_type::VectorValue;
use crate::coprocessor::codec::Result;

/// Stores multiple `LazyBatchColumn`s. Each column has an equal length.
#[derive(Clone, Debug)]
pub struct LazyBatchColumnVec {
    /// Multiple lazy batch columns. Each column is either decoded, or not decoded.
    ///
    /// For decoded columns, they may be in different types. If the column is in
    /// type `LazyBatchColumn::Raw`, it means that it is not decoded.
    columns: Vec<LazyBatchColumn>,
}

impl From<Vec<LazyBatchColumn>> for LazyBatchColumnVec {
    #[inline]
    fn from(columns: Vec<LazyBatchColumn>) -> Self {
        LazyBatchColumnVec { columns }
    }
}

impl From<Vec<VectorValue>> for LazyBatchColumnVec {
    #[inline]
    fn from(columns: Vec<VectorValue>) -> Self {
        LazyBatchColumnVec {
            columns: columns
                .into_iter()
                .map(|v| LazyBatchColumn::from(v))
                .collect(),
        }
    }
}

impl LazyBatchColumnVec {
    /// Creates a new empty `LazyBatchColumnVec`, which does not have columns and rows.
    ///
    /// Because column numbers won't change, it means constructed instance will be always empty.
    #[inline]
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
        }
    }

    /// Creates a new `LazyBatchColumnVec`, which contains `columns_count` number of raw columns.
    #[cfg(test)]
    pub fn with_raw_columns(columns_count: usize) -> Self {
        let mut columns = Vec::with_capacity(columns_count);
        for _ in 0..columns_count {
            let column = LazyBatchColumn::raw_with_capacity(0);
            columns.push(column);
        }
        Self { columns }
    }

    /// Returns the number of columns.
    ///
    /// It might be possible that there is no row but multiple columns.
    #[inline]
    pub fn columns_len(&self) -> usize {
        self.columns.len()
    }

    /// Returns the number of rows.
    #[inline]
    pub fn rows_len(&self) -> usize {
        if self.columns.is_empty() {
            return 0;
        }
        self.columns[0].len()
    }

    /// Asserts that all columns have equal length.
    #[inline]
    pub fn assert_columns_equal_length(&self) {
        let len = self.rows_len();
        for column in &self.columns {
            assert_eq!(len, column.len());
        }
    }

    /// Retains the elements according to a boolean array.
    ///
    /// # Panics
    ///
    /// Panics if `retain_arr` is not long enough.
    pub fn retain_rows_by_array(&mut self, retain_arr: &[bool]) {
        if self.rows_len() == 0 {
            return;
        }

        let current_rows_len = self.rows_len();

        // We retain column by column to be efficient.
        for col in &mut self.columns {
            assert_eq!(col.len(), current_rows_len);
            col.retain_by_array(retain_arr);
        }

        self.assert_columns_equal_length();
    }

    /// Returns maximum encoded size.
    pub fn maximum_encoded_size(&self, output_offsets: impl AsRef<[u32]>) -> Result<usize> {
        let mut size = 0;
        for offset in output_offsets.as_ref() {
            size += self.columns[(*offset) as usize].maximum_encoded_size()?;
        }
        Ok(size)
    }

    /// Encodes into binary format.
    pub fn encode(
        &self,
        output_offsets: impl AsRef<[u32]>,
        schema: impl AsRef<[FieldType]>,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        let len = self.rows_len();
        let schema = schema.as_ref();
        for i in 0..len {
            for offset in output_offsets.as_ref() {
                let offset = *offset as usize;
                let col = &self.columns[offset];
                col.encode(i, &schema[offset], output)?;
            }
        }
        Ok(())
    }

    /// Truncates columns into equal length. The new length of all columns would be the length of
    /// the shortest column before calling this function.
    pub fn truncate_into_equal_length(&mut self) {
        let mut min_len = self.rows_len();
        for col in &self.columns {
            min_len = min_len.min(col.len());
        }
        self.truncate(min_len);
    }

    /// Shortens the rows, keeping the first `len` rows and dropping the rest.
    pub fn truncate(&mut self, len: usize) {
        for col in &mut self.columns {
            col.truncate(len);
        }
        self.assert_columns_equal_length();
    }
}

impl std::ops::Deref for LazyBatchColumnVec {
    type Target = [LazyBatchColumn];

    #[inline]
    fn deref(&self) -> &[LazyBatchColumn] {
        self.columns.deref()
    }
}

impl std::ops::DerefMut for LazyBatchColumnVec {
    #[inline]
    fn deref_mut(&mut self) -> &mut [LazyBatchColumn] {
        self.columns.deref_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cop_datatype::EvalType;

    use crate::coprocessor::codec::data_type::Real;
    use crate::coprocessor::codec::datum::{Datum, DatumEncoder};
    use crate::coprocessor::codec::mysql::Tz;

    /// Pushes a raw row. There must be no empty datum.
    ///
    /// # Panic
    ///
    /// Panics if there is empty datum.
    ///
    /// Panics if the length of row does not match the size of the columns vector.
    ///
    /// Panics if some column in the vector is decoded.
    fn push_raw_row<D, V>(columns: &mut LazyBatchColumnVec, raw_row: V)
    where
        D: AsRef<[u8]>,
        V: AsRef<[D]>,
    {
        let raw_row_slice = raw_row.as_ref();
        assert_eq!(columns.columns_len(), raw_row_slice.len());
        for (col_index, raw_datum) in raw_row_slice.iter().enumerate() {
            let lazy_col = &mut columns.columns[col_index];
            assert!(lazy_col.is_raw());
            lazy_col.mut_raw().push(raw_datum);
        }

        columns.assert_columns_equal_length();
    }

    /// Pushes a raw row via a datum vector.
    fn push_raw_row_from_datums(
        columns: &mut LazyBatchColumnVec,
        datums: impl AsRef<[Datum]>,
        comparable: bool,
    ) {
        let raw_row: Vec<_> = datums
            .as_ref()
            .iter()
            .map(|some_datum| {
                let mut ret = Vec::new();
                DatumEncoder::encode(&mut ret, &[some_datum.clone()], comparable).unwrap();
                ret
            })
            .collect();
        push_raw_row(columns, raw_row);
    }

    #[test]
    fn test_ensure_column_decoded() {
        use cop_datatype::FieldTypeTp;

        for comparable in &[true, false] {
            let schema = [
                FieldTypeTp::Long.into(),
                FieldTypeTp::Double.into(),
                FieldTypeTp::VarChar.into(),
            ];
            let values = vec![
                vec![Datum::U64(1), Datum::F64(1.0), Datum::Null],
                vec![Datum::Null, Datum::Null, Datum::Bytes(vec![0u8, 2u8])],
            ];

            // Empty LazyBatchColumnVec
            let mut columns = LazyBatchColumnVec::with_raw_columns(3);
            assert_eq!(columns.rows_len(), 0);
            assert_eq!(columns.columns_len(), 3);

            for raw_datum in &values {
                push_raw_row_from_datums(&mut columns, raw_datum, *comparable);
            }
            assert_eq!(columns.rows_len(), values.len());

            // Decode Column Index 2
            assert!(!columns[2].is_decoded());
            {
                columns[2].ensure_decoded(&Tz::utc(), &schema[2]).unwrap();
                let col = columns[2].decoded();
                assert_eq!(col.len(), 2);
                assert_eq!(col.eval_type(), EvalType::Bytes);
                assert_eq!(col.as_bytes_slice(), &[None, Some(vec![0u8, 2u8])]);
            }
            // Decode a decoded column
            assert!(columns[2].is_decoded());
            {
                columns[2].ensure_decoded(&Tz::utc(), &schema[2]).unwrap();
                let col = columns[2].decoded();
                assert_eq!(col.len(), 2);
                assert_eq!(col.eval_type(), EvalType::Bytes);
                assert_eq!(col.as_bytes_slice(), &[None, Some(vec![0u8, 2u8])]);
            }
            assert!(columns[2].is_decoded());

            // Decode Column Index 0
            assert!(!columns[0].is_decoded());
            {
                columns[0].ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
                let col = columns[0].decoded();
                assert_eq!(col.len(), 2);
                assert_eq!(col.eval_type(), EvalType::Int);
                assert_eq!(col.as_int_slice(), &[Some(1), None]);
            }
            assert!(columns[0].is_decoded());

            // Decode Column Index 1
            assert!(!columns[1].is_decoded());
            {
                columns[1].ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
                let col = columns[1].decoded();
                assert_eq!(col.len(), 2);
                assert_eq!(col.eval_type(), EvalType::Real);
                assert_eq!(col.as_real_slice(), &[Real::new(1.0).ok(), None]);
            }
            assert!(columns[1].is_decoded());
        }
    }

    #[test]
    fn test_retain_rows_by_array() {
        use cop_datatype::FieldTypeTp;

        let schema = [FieldTypeTp::Long.into(), FieldTypeTp::Double.into()];
        let mut columns = LazyBatchColumnVec::with_raw_columns(2);
        assert_eq!(columns.rows_len(), 0);
        assert_eq!(columns.columns_len(), 2);
        columns.retain_rows_by_array(&[]);
        assert_eq!(columns.rows_len(), 0);
        assert_eq!(columns.columns_len(), 2);
        columns.retain_rows_by_array(&[true]);
        assert_eq!(columns.rows_len(), 0);
        assert_eq!(columns.columns_len(), 2);
        columns.retain_rows_by_array(&[false]);
        assert_eq!(columns.rows_len(), 0);
        assert_eq!(columns.columns_len(), 2);

        push_raw_row_from_datums(&mut columns, &[Datum::Null, Datum::F64(1.3)], false);
        push_raw_row_from_datums(&mut columns, &[Datum::Null, Datum::Null], false);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(3), Datum::Null], true);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(3), Datum::F64(5.0)], false);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(11), Datum::F64(7.5)], true);
        push_raw_row_from_datums(&mut columns, &[Datum::Null, Datum::F64(13.1)], true);

        columns.retain_rows_by_array(&[true, true, false, false, true, false]);

        assert_eq!(columns.rows_len(), 3);
        assert_eq!(columns.columns_len(), 2);
        {
            let mut column0 = columns[0].clone();
            assert!(column0.is_raw());
            column0.ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
            assert_eq!(column0.decoded().len(), 3);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(column0.decoded().as_int_slice(), &[None, None, Some(11)]);
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 3);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[Real::new(1.3).ok(), None, Real::new(7.5).ok()]
            );
        }

        push_raw_row_from_datums(&mut columns, &[Datum::Null, Datum::F64(101.51)], false);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(1), Datum::Null], false);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(5), Datum::F64(1.9)], true);
        push_raw_row_from_datums(&mut columns, &[Datum::Null, Datum::F64(101.51)], false);

        assert_eq!(columns.rows_len(), 7);
        assert_eq!(columns.columns_len(), 2);
        {
            let mut column0 = columns[0].clone();
            assert!(column0.is_raw());
            column0.ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
            assert_eq!(column0.decoded().len(), 7);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(
                column0.decoded().as_int_slice(),
                &[None, None, Some(11), None, Some(1), Some(5), None]
            );
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 7);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[
                    Real::new(1.3).ok(),
                    None,
                    Real::new(7.5).ok(),
                    Real::new(101.51).ok(),
                    None,
                    Real::new(1.9).ok(),
                    Real::new(101.51).ok()
                ]
            );
        }

        columns.retain_rows_by_array(&[true, false, true, false, false, true, true]);

        assert_eq!(columns.rows_len(), 4);
        assert_eq!(columns.columns_len(), 2);
        {
            let mut column0 = columns[0].clone();
            assert!(column0.is_raw());
            column0.ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
            assert_eq!(column0.decoded().len(), 4);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(
                column0.decoded().as_int_slice(),
                &[None, Some(11), Some(5), None]
            );
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 4);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[
                    Real::new(1.3).ok(),
                    Real::new(7.5).ok(),
                    Real::new(1.9).ok(),
                    Real::new(101.51).ok()
                ]
            );
        }

        columns.retain_rows_by_array(&[true, true, true, true]);

        assert_eq!(columns.rows_len(), 4);
        assert_eq!(columns.columns_len(), 2);
        {
            let mut column0 = columns[0].clone();
            assert!(column0.is_raw());
            column0.ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
            assert_eq!(column0.decoded().len(), 4);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(
                column0.decoded().as_int_slice(),
                &[None, Some(11), Some(5), None]
            );
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 4);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[
                    Real::new(1.3).ok(),
                    Real::new(7.5).ok(),
                    Real::new(1.9).ok(),
                    Real::new(101.51).ok()
                ]
            );
        }

        columns.retain_rows_by_array(&[false, false, false, false]);

        assert_eq!(columns.rows_len(), 0);
        assert_eq!(columns.columns_len(), 2);
        {
            let mut column0 = columns[0].clone();
            assert!(column0.is_raw());
            column0.ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
            assert_eq!(column0.decoded().len(), 0);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(column0.decoded().as_int_slice(), &[]);
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 0);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(column1.decoded().as_real_slice(), &[]);
        }

        push_raw_row_from_datums(&mut columns, &[Datum::Null, Datum::F64(7.77)], true);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(5), Datum::Null], false);
        push_raw_row_from_datums(&mut columns, &[Datum::U64(1), Datum::F64(7.17)], false);

        assert_eq!(columns.rows_len(), 3);
        assert_eq!(columns.columns_len(), 2);
        {
            let mut column0 = columns[0].clone();
            assert!(column0.is_raw());
            column0.ensure_decoded(&Tz::utc(), &schema[0]).unwrap();
            assert_eq!(column0.decoded().len(), 3);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(column0.decoded().as_int_slice(), &[None, Some(5), Some(1)]);
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 3);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[Real::new(7.77).ok(), None, Real::new(7.17).ok()]
            );
        }

        // Let's change a column from lazy to decoded and test whether retain works
        columns[0].ensure_decoded(&Tz::utc(), &schema[0]).unwrap();

        columns.retain_rows_by_array(&[true, true, true]);

        assert_eq!(columns.rows_len(), 3);
        assert_eq!(columns.columns_len(), 2);
        {
            let column0 = &columns[0];
            assert!(column0.is_decoded());
            assert_eq!(column0.decoded().len(), 3);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(column0.decoded().as_int_slice(), &[None, Some(5), Some(1)]);
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 3);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[Real::new(7.77).ok(), None, Real::new(7.17).ok()]
            );
        }

        columns.retain_rows_by_array(&[true, false, true]);

        assert_eq!(columns.rows_len(), 2);
        assert_eq!(columns.columns_len(), 2);
        {
            let column0 = &columns[0];
            assert!(column0.is_decoded());
            assert_eq!(column0.decoded().len(), 2);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(column0.decoded().as_int_slice(), &[None, Some(1)]);
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 2);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(
                column1.decoded().as_real_slice(),
                &[Real::new(7.77).ok(), Real::new(7.17).ok()]
            );
        }

        columns.retain_rows_by_array(&[false, false]);

        assert_eq!(columns.rows_len(), 0);
        assert_eq!(columns.columns_len(), 2);
        {
            let column0 = &columns[0];
            assert!(column0.is_decoded());
            assert_eq!(column0.decoded().len(), 0);
            assert_eq!(column0.decoded().eval_type(), EvalType::Int);
            assert_eq!(column0.decoded().as_int_slice(), &[]);
        }
        {
            let mut column1 = columns[1].clone();
            assert!(column1.is_raw());
            column1.ensure_decoded(&Tz::utc(), &schema[1]).unwrap();
            assert_eq!(column1.decoded().len(), 0);
            assert_eq!(column1.decoded().eval_type(), EvalType::Real);
            assert_eq!(column1.decoded().as_real_slice(), &[]);
        }
    }
}
